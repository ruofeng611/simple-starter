# simple-starter-security

Security 模块为 simple-starter 提供编译期资源收集、运行时白名单放行、用户认证与权限校验等安全能力。

## 核心特性

- **双层拦截**：白名单路径放行 + 基于资源标识的权限校验
- **编译期收集**：通过 `inventory` 在编译期静态收集安全资源，运行时零反射开销
- **灵活扩展**：所有核心行为均为 trait，支持自定义用户信息解析、权限检查、错误响应等
- **Axum 原生集成**：以 Axum 中间件形式注入，支持 `MatchedPath` 路由模式匹配
- **基础路径自动拼接**：自动适配 `web.base_path` 配置，无需手动修改资源路径

## 快速开始

### 1. 添加 SecurityPlugin

```rust
use simple_starter_core::Application;
use simple_starter_security::{SecurityPlugin, UserContext, UserInfoProvider};
use simple_starter_web::WebPlugin;

fn main() {
    Application::new()
        .register_plugin(
            SecurityPlugin::new()
                .with_user_info_provider(MyUserInfoProvider)
        )
        .register_plugin(WebPlugin::new())
        .run();
}
```

### 2. 在 Controller 上标记安全资源

```rust
use simple_starter_macro::{rest_controller, post_mapping, security_controller, security_resource};

#[security_controller]
#[rest_controller("/test")]
impl TestController {
    #[post_mapping("/student/add")]
    #[security_resource]
    pub async fn add_student(&self) -> JsonResponse {
        // 只有拥有该资源权限的用户才能访问
        JsonResponse::ok(())
    }
}
```

### 3. 在 Handler 中获取用户上下文

```rust
use simple_starter_web::axum::extract;
use simple_starter_security::UserContext;

pub async fn handler(
    extract::Extension(user_ctx): extract::Extension<UserContext>,
) -> JsonResponse {
    println!("user_id: {}", user_ctx.user_id);
    println!("has resource: {}", user_ctx.has_resource("test::add_student"));
    JsonResponse::ok(())
}
```

## 核心概念

### SecurityPlugin

`SecurityPlugin` 是安全模块的入口，作为 `Plugin` 注册到 `Application` 中。

```rust
SecurityPlugin::new()
    .with_user_info_provider(MyUserInfoProvider)   // 设置用户信息提供者（可选，但强烈建议）
    .with_permission_checker(MyPermissionChecker) // 自定义权限检查器（可选）
    .with_error_handler(MyErrorHandler)           // 自定义错误响应（可选）
    .with_base_path_provider(MyBasePathProvider)  // 自定义基础路径提供者（可选）
    .add_whitelist(Some("GET"), "/public/*")      // 添加白名单
```

### 资源标识（ResourceEntry）

资源标识通过属性宏在编译期注册，运行时由 `SecurityPlugin` 收集并构建 `path_pattern -> resource_id` 映射表。

| 宏 | 作用位置 | 说明 |
|---|---|---|
| `#[security]` | 自由函数 | 必须配合 `#[get]`/`#[post]`/`#[put]`/`#[delete]` 使用 |
| `#[security_controller]` | `impl` 块 | 必须放在 `#[rest_controller]` 外层（属性宏执行顺序：外→内） |
| `#[security_resource]` | `impl` 方法 | 仅显式标记的方法才会注册资源信息 |

### 用户上下文（UserContext）

```rust
pub struct UserContext {
    pub user_id: String,           // 用户唯一标识
    pub resource_ids: Vec<String>, // 拥有的资源标识列表
    pub is_disabled: bool,         // 是否被禁用
    pub expired_at: Option<SystemTime>, // 过期时间
    pub extra: Option<Value>,      // 扩展字段
}

impl UserContext {
    pub fn has_resource(&self, resource_id: &str) -> bool;
    pub fn is_expired(&self) -> bool;   // None 表示永不过期
    pub fn is_active(&self) -> bool;    // !is_disabled && !is_expired()
}
```

### 权限检查器（PermissionChecker）

默认实现 `DefaultPermissionChecker` 直接调用 `user_ctx.has_resource(resource_id)` 判断。

```rust
#[async_trait::async_trait]
pub trait PermissionChecker: Send + Sync {
    async fn check(&self, user_ctx: &UserContext, resource_id: &str) -> bool;
}
```

你可以通过 `with_permission_checker` 提供自定义实现（如基于 RBAC、ABAC 的复杂逻辑）。

### 错误处理器（SecurityErrorHandler）

默认返回标准 HTTP 401/403。你可以自定义返回 JSON 格式的业务错误响应：

```rust
#[async_trait::async_trait]
pub trait SecurityErrorHandler: Send + Sync {
    async fn unauthorized(&self, parts: &http::request::Parts) -> Response;
    async fn forbidden(&self, parts: &http::request::Parts, error: &SecurityError) -> Response;
}
```

> **注意**：参数使用 `&Parts` 而非 `&Request<Body>`，以规避 `Body` 非 `Sync` 导致 `async_trait` Future 非 `Send` 的陷阱。

## 配置说明

### security.log_warn

控制安全中间件是否打印警告日志（如用户禁用、资源未找到、权限不足等）。

```toml
[security]
log_warn = true  # 默认 true，生产环境可设为 false 减少噪音
```

### web.base_path

`DefaultBasePathProvider` 自动从 `web.base_path` 读取基础路径，并在构建资源映射表时自动拼接：

```toml
[web]
base_path = "/api"
```

若 Controller 注册路径为 `/test/student/add`，则中间件实际匹配 `/api/test/student/add`。

## 高级扩展

### 自定义 UserInfoProvider

```rust
use simple_starter_security::{UserContext, UserInfoProvider};
use simple_starter_web::axum::http;

pub struct JwtUserInfoProvider;

#[async_trait::async_trait]
impl UserInfoProvider for JwtUserInfoProvider {
    async fn get_user_context(&self, parts: &http::request::Parts) -> Option<UserContext> {
        let token = parts.headers.get("Authorization")?;
        // 解析 JWT，构造 UserContext
        Some(UserContext { /* ... */ })
    }
}
```

### 自定义 SecurityErrorHandler（JSON 响应）

```rust
use simple_starter_security::{SecurityError, SecurityErrorHandler};
use simple_starter_web::axum::{http, response::IntoResponse, Json};

pub struct JsonSecurityErrorHandler;

#[async_trait::async_trait]
impl SecurityErrorHandler for JsonSecurityErrorHandler {
    async fn unauthorized(&self, parts: &http::request::Parts) -> axum::response::Response {
        let resp = JsonResponse {
            code: 401,
            message: "未认证".to_string(),
            data: Some(json!({"path": parts.uri.path()})),
            ..Default::default()
        };
        (http::StatusCode::UNAUTHORIZED, Json(resp)).into_response()
    }

    async fn forbidden(&self, parts: &http::request::Parts, error: &SecurityError) -> axum::response::Response {
        let resp = JsonResponse {
            code: 403,
            message: format!("权限不足: {}", error),
            data: Some(json!({"path": parts.uri.path(), "detail": format!("{:?}", error)})),
            ..Default::default()
        };
        (http::StatusCode::FORBIDDEN, Json(resp)).into_response()
    }
}
```

### 自定义 BasePathProvider

```rust
use simple_starter_security::BasePathProvider;

pub struct FixedBasePathProvider;

impl BasePathProvider for FixedBasePathProvider {
    fn base_path(&self) -> String {
        "/v2".to_string()
    }
}
```

## 设计要点

### 编译期资源收集

资源通过 `inventory::submit!` 在编译期注册，`SecurityPlugin::collect_resources()` 可在任何时刻获取：

```rust
let resources = SecurityPlugin::collect_resources();
for entry in resources {
    println!("{} -> {}", entry.path_pattern, entry.resource_id);
}
```

### 运行时路径拼接

`init` 阶段读取 `base_path_provider.base_path()`，将 `/api` 与 `/test/student/add` 拼接为 `/api/test/student/add`，确保 `MatchedPath` 与 `resource_map` key 精确匹配。

### 白名单机制

白名单在权限校验之前执行，支持方法级别和通配符路径：

```rust
SecurityPlugin::new()
    .add_whitelist(Some("GET"), "/public/*")
    .add_whitelist(None, "/health")  // None 表示所有 HTTP 方法
```

### 错误类型

```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum SecurityError {
    #[error("User '{user_id}' is disabled")]
    UserDisabled { user_id: String },
    #[error("User '{user_id}' has expired")]
    UserExpired { user_id: String },
    #[error("MatchedPath not available")]
    MatchedPathUnavailable,
    #[error("No resource registered for path pattern '{pattern}'")]
    ResourceNotFound { pattern: String },
    #[error("User '{user_id}' denied access to resource '{resource_id}'")]
    PermissionDenied { user_id: String, resource_id: String },
}
```

所有错误均通过 `SecurityErrorHandler` 处理，你可以根据具体错误类型返回不同的业务状态码或响应体。
