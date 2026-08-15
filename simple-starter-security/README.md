# simple-starter-security

`simple-starter-security` 是 simple-starter 的**安全插件模块**：提供编译期资源收集、运行时白名单放行、用户认证与权限校验能力，以 Axum 中间件形式集成到 Web 服务，并重导出全部安全宏。

## 一、基本原理

### 1. 编译期资源收集

安全资源（接口的权限元数据）在**编译期**通过 `inventory` 静态收集：

1. `#[security]` / `#[security_controller]` + `#[security_resource]` 宏展开为 `ResourceEntry`（`path_pattern` + `resource_id` + `resource_name` + 模块信息）并 `submit!` 注册。
2. `SecurityPlugin::components_ready` 阶段遍历 inventory 收集全部条目，构建 `path_pattern -> resource_id` 映射表（运行时校验路径唯一性）。
3. 运行期权限校验时，Axum `MatchedPath` 提供实际匹配的路由模式，直接查表得到 `resource_id`，零反射开销。

### 2. 中间件执行流程

安全中间件在业务 handler 之前拦截请求，按序执行：

1. 白名单检查：命中白名单直接放行
2. 解析用户上下文：`UserInfoProvider` 从请求头/令牌解析，失败返回 401
3. 用户状态检查：禁用或过期返回 403
4. 权限检查：`MatchedPath` → `resource_id` → `PermissionChecker::check`，不通过返回 403
5. 放行：把 `UserContext` 附加到 Request extensions（handler 经 `extract::Extension<UserContext>` 获取）

### 3. 基础路径拼接

`BasePathProvider`（默认读 `web.base_path`）提供基础路径，`components_ready` 阶段将其与资源路径拼接（如 `/api` + `/test/student/add`），保证 `MatchedPath` 与资源映射表 key 精确匹配。

### 4. 协作接口与覆盖语义

四个协作接口均通过**组件仓库**获取（trait 对象注入）：

| 接口 | 默认实现 | 未提供时的行为 |
|---|---|---|
| `UserInfoProvider` | 无 | **拒绝所有请求**（必须由用户提供） |
| `PermissionChecker` | 有（校验 `resource_ids` 集合） | 自动使用默认实现 |
| `SecurityErrorHandler` | 有（返回标准 401/403） | 自动使用默认实现 |
| `BasePathProvider` | 有（读 `web.base_path`） | 自动使用默认实现 |

默认实现以 `on_missing_trait` 条件注册：用户注册自定义实现时自动退位，未注册时生效。

## 二、导出的用户可用组件与宏

### 1. SecurityPlugin（插件入口）

```rust
use simple_starter_core::Application;
use simple_starter_security::SecurityPlugin;
use simple_starter_web::WebPlugin;

fn main() {
    Application::new()
        .register_plugin(WebPlugin::new())     // SecurityPlugin 依赖 WebPlugin（自动拓扑排序）
        .register_plugin(SecurityPlugin::new()
            .add_whitelist(Some("GET"), "/health")   // 精确匹配
            .add_whitelist(None, "/public/*"))       // 前缀匹配，None = 所有方法
        .run();
}
```

| 方法 | 说明 |
|---|---|
| `new()` | 创建插件 |
| `add_whitelist(method, path)` | 添加白名单：`method` 为 `None` 匹配所有方法；`path` 以 `/*` 结尾为前缀匹配，否则精确匹配 |
| `collect_resources()` | 静态方法，获取编译期收集的全部 `ResourceEntry`（任意时刻可调用） |

### 2. 安全宏

#### `#[security_controller]` + `#[security_resource]`（impl 块形式）

`#[security_controller]` 必须放在 `#[rest_controller]` **外层**（属性宏执行顺序：外 → 内）；仅显式标记 `#[security_resource]` 的方法才注册资源：

```rust
use simple_starter_security::{security_controller, security_resource};

#[security_controller]
#[rest_controller("/test")]
impl TestController {
    #[post_mapping("/student/add")]
    #[security_resource]
    pub async fn add_student(&self) -> JsonResponse { /* 受保护资源 */ }
}
```

资源标识默认为 `Controller名::方法名`（如 `TestController::add_student`），可用 `#[security_resource(resource_id = "...", resource_name = "...")]` 覆盖。

#### `#[security]`（自由函数形式）

作用于自由函数，必须搭配 `#[get]` / `#[post]` / `#[put]` / `#[delete]` 使用：

```rust
#[security(resource_id = "student_query", resource_name = "学生查询")]
#[get("/student/{id}")]
#[json_response]
async fn get_student(axum::extract::Path(id): axum::extract::Path<i64>) -> JsonResponse { /* ... */ }
```

### 3. UserContext（用户上下文）

由 `UserInfoProvider` 构造，经中间件附加到请求，handler 通过 `extract::Extension<UserContext>` 获取：

```rust
pub struct UserContext {
    pub user_id: String,                        // 用户唯一标识
    pub resource_ids: HashSet<String>,          // 拥有的资源标识集合
    pub is_disabled: bool,                      // 是否被禁用
    pub expired_at: Option<std::time::SystemTime>, // 过期时间（None = 永不过期）
    pub extra: Option<serde_json::Value>,       // 业务自定义扩展字段
}

impl UserContext {
    pub fn has_resource(&self, resource_id: &str) -> bool;
    pub fn is_expired(&self) -> bool;
    pub fn is_active(&self) -> bool;   // !is_disabled && !is_expired()
}
```

### 4. SecurityError（安全错误类型）

中间件产生的所有错误均经 `SecurityErrorHandler` 精确处理：

| 变体 | 场景 |
|---|---|
| `UserDisabled { user_id }` | 用户被禁用 |
| `UserExpired { user_id }` | 会话已过期 |
| `MatchedPathUnavailable` | 无法获取路由匹配模式 |
| `ResourceNotFound { pattern }` | 路径未注册对应资源 |
| `PermissionDenied { user_id, resource_id }` | 权限校验不通过 |

## 三、扩展点（协作接口自定义实现）

### 1. UserInfoProvider（必填）

无默认实现，必须注册自定义组件（否则所有请求被拒绝）：

```rust
#[component]
pub struct JwtUserInfoProvider;

#[injectable]
#[async_trait::async_trait]
impl UserInfoProvider for JwtUserInfoProvider {
    async fn get_user_context(&self, parts: &http::request::Parts) -> Option<UserContext> {
        let user_id = parts.headers.get("user-id")?.to_str().ok()?.to_string();
        Some(UserContext {
            user_id,
            resource_ids: HashSet::new(),
            is_disabled: false,
            expired_at: None,
            extra: None,
        })
    }
}
```

### 2. PermissionChecker（可选，自定义校验逻辑）

```rust
#[component]
pub struct MyPermissionChecker;

#[injectable]
#[async_trait::async_trait]
impl PermissionChecker for MyPermissionChecker {
    async fn check(&self, user_ctx: &UserContext, resource_id: &str) -> bool {
        // 基于 RBAC / ABAC 的自定义逻辑
        user_ctx.has_resource(resource_id)
    }
}
```

### 3. SecurityErrorHandler（可选，自定义错误响应）

```rust
#[component]
pub struct JsonSecurityErrorHandler;

#[injectable]
#[async_trait::async_trait]
impl SecurityErrorHandler for JsonSecurityErrorHandler {
    async fn unauthorized(&self, parts: &http::request::Parts) -> axum::response::Response {
        let resp = JsonResponse {
            code: 401,
            message: "未认证，请登录后访问".to_string(),
            data: Some(serde_json::json!({ "path": parts.uri.path() })),
            ..Default::default()
        };
        (http::StatusCode::UNAUTHORIZED, axum::Json(resp)).into_response()
    }

    async fn forbidden(&self, parts: &http::request::Parts, error: &SecurityError) -> axum::response::Response {
        let resp = JsonResponse {
            code: 403,
            message: format!("权限不足: {}", error),
            data: Some(serde_json::json!({ "detail": format!("{:?}", error) })),
            ..Default::default()
        };
        (http::StatusCode::FORBIDDEN, axum::Json(resp)).into_response()
    }
}
```

### 4. BasePathProvider（可选，自定义基础路径）

```rust
#[component]
pub struct FixedBasePathProvider;

#[injectable]
impl BasePathProvider for FixedBasePathProvider {
    fn base_path(&self) -> String { "/v2".to_string() }
}
```

## 四、组合使用示例

以下示例串联用户上下文解析、自定义 JSON 错误响应、受保护资源与启动钩子初始化权限缓存：

```rust
use simple_starter_core::{component, injectable, Application, anyhow};
use simple_starter_security::{SecurityPlugin, UserContext, UserInfoProvider, SecurityErrorHandler, SecurityError};
use simple_starter_web::WebPlugin;
use std::collections::HashSet;

// 1. 自定义用户上下文提供者（从请求头 user-id 解析）
#[component]
pub struct UserInfoProviderImpl;
#[injectable]
#[async_trait::async_trait]
impl UserInfoProvider for UserInfoProviderImpl {
    async fn get_user_context(&self, parts: &http::request::Parts) -> Option<UserContext> {
        let user_id = parts.headers.get("user-id")?.to_str().ok()?.to_string();
        Some(UserContext {
            user_id,
            resource_ids: HashSet::new(),
            is_disabled: false,
            expired_at: None,
            extra: None,
        })
    }
}

fn main() {
    Application::new()
        .register_plugin(WebPlugin::new())
        .register_plugin(SecurityPlugin::new()
            .add_whitelist(Some("GET"), "/health"))
        // 启动钩子：组件就绪后初始化权限数据（user_id "1" 拥有全部资源权限）
        .add_startup_hook(async {
            let resources = SecurityPlugin::collect_resources();
            // 把全部 resource_id 授权给 user_id "1" ...
            Ok(())
        })
        .run();
}
```

## 五、配置项（`[security]` 节点）

```toml
[security]
log_warn = true   # 是否打印安全警告日志（用户禁用、资源未找到、权限不足等）
```
