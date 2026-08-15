# simple-starter-web

`simple-starter-web` 是 simple-starter 的 **Web 插件模块**：集成 Axum 框架，提供路由自动收集、REST 控制器、统一 JSON 响应与监听器扩展，并重导出全部 Web 宏。

## 一、基本原理

### 1. 分布式路由自动收集

路由定义分散在各模块的 Controller 中，启动时**自动收集聚合**，无需在 `main` 集中挂载：

1. 路由宏（`#[get]`、`#[rest_controller]` 等）在编译期把 handler 包装为 `RouteFactory`（`fn() -> Router`），通过 `inventory` 静态收集。
2. `WebPlugin::finalize` 阶段消费 `WebExtensionRegistry`，注册延迟构建的后台任务。
3. 服务启动时（`build_and_serve`）遍历 inventory 中全部 `RouteFactory`，`router.merge(...)` 合并为完整 Router。

### 2. 分层构建

最终 Router 按固定顺序分层构建（`build_and_serve`）：

1. 合并自动收集与手动注册的路由
2. 应用路由修改器（扩展点）
3. 挂载 `base_path`（如 `/api`）
4. 应用外部中间件（业务层，扩展点）
5. 应用框架自带 `TraceLayer`（日志追踪，`logger.level` 控制级别）
6. 构建监听器（`TcpListenerFactory`，可自定义 TLS/UDS）
7. 启动 `axum::serve`，优雅退出监听 `CancellationToken`

### 3. 插件生命周期

| 周期 | 行为 |
|---|---|
| `assemble` | 创建 `WebExtensionRegistry` 并放入 `Application` 扩展上下文，供其他插件注册中间件、路由修改器 |
| `finalize` | 所有组件就绪后：加载 `web` 配置、取出注册表、从组件仓库获取 `TcpListenerFactory`、注册服务后台任务 |

### 4. Controller 参数重写原理

`#[rest_controller]` 把方法**改写为两段**：原方法保留（Axum 提取器参数重写为裸类型，如 `Path(id): Path<i64>` → `id: i64`），另生成一个路由 handler（保留提取器参数形式）负责从 `State<Arc<Controller>>` 取组件并调用原方法，返回值自动用 `Json<T>` 包裹。

## 二、导出的用户可用组件与宏

### 1. WebPlugin（插件入口）

```rust
use simple_starter_core::Application;
use simple_starter_web::{WebPlugin, axum};

fn main() {
    Application::new()
        .register_plugin(WebPlugin::new()
            .add_manual_router_factory(|| axum::Router::new().route("/manual", axum::routing::get(manual_handler)))
            .add_middleware(|router| router.layer(CompressionLayer::new()))
            .add_router_modifier(|router| router.fallback(fallback_handler))
            .set_server_scheme("https"))
        .run();
}
```

| 方法 | 说明 |
|---|---|
| `new()` | 创建插件 |
| `add_manual_router_factory(f)` | 手动挂载动态构建的路由（自动收集之外的补充） |
| `add_router_modifier(f)` | 注册路由修改器（所有路由合并后、`base_path` 前调用） |
| `add_middleware(f)` | 注册中间件（`base_path` 后、框架 `TraceLayer` 前执行） |
| `set_server_scheme(s)` | 设置协议前缀（如 `"https"`，影响启动日志） |

### 2. 自由函数路由宏：`#[get]` / `#[post]` / `#[put]` / `#[delete]`

参数支持简写 `#[get("/path")]` 与键值 `#[get(path = "/path", state = expr)]`：

```rust
#[get(path = "/student/{id}", state = AppCoreUtil::get_component::<StudentService>().unwrap())]
#[json_response]  // 自动将返回值包装为 Json
async fn get_student_name(
    axum::extract::Path(id): axum::extract::Path<i64>,
    State(student_service): State<Arc<StudentService>>,
) -> JsonResponse {
    json_response_wrap!(function_name = "根据学生id获取学生姓名", {
        if id == 0 {
            return Err(SimpleAppWebError::new(400, "无效的学生id"));
        }
        Ok(student_service.get_student_name(id).await
            .ok_or_else(|| SimpleAppWebError::new(404, "未找到该id相关的学生姓名"))?)
    })
}
```

### 3. REST 控制器宏：`#[rest_controller]` + `*_mapping`

`#[rest_controller("/api")]` 声明基础路径；方法级 `#[get_mapping]` / `#[post_mapping]` / `#[put_mapping]` / `#[delete_mapping]` 标记路由（路径参数简写或键值均可）。Controller 本身是组件，可注入依赖：

```rust
#[component]
pub struct TestController {
    #[inject]
    student_service: Arc<StudentService>,
}

#[rest_controller("/test")]
impl TestController {
    #[post_mapping("/student/add")]
    pub async fn add_student(
        &self,
        extract::Json(student): extract::Json<StudentDto>,
    ) -> JsonResponse {
        json_response_wrap!(function_name = "添加学生", {
            self.student_service.add(student).await?;
            Ok(())
        })
    }
}
```

### 4. `#[json_response]` 宏

作用于 `async fn`，把返回类型 `T` 自动包装为 `axum::Json<T>`，省去手动包裹。

### 5. `JsonResponse` 与 `json_response_wrap!` 宏

`JsonResponse` 是标准响应结构（`{ code, message, service_name, function_name, data }`，camelCase 序列化）。`json_response_wrap!` 执行异步代码块并把 `Result<T, SimpleAppWebError>` 转换为 `JsonResponse`：

- 成功：`code`/`message` 使用宏参数（默认 200 / "操作成功"），`data` 序列化业务返回值
- 失败：使用 `SimpleAppWebError` 自带的 `code`/`message`/`data`，自动记录错误链日志

支持模式：`json_response_wrap!(code = ..., message = ..., function_name = ..., { ... })`（任意组合）。

### 6. `SimpleAppWebError`（业务错误）

```rust
SimpleAppWebError::new(400, "无效的学生id")
    .with_data(json!({ "field": "id" }))
    .with_source(io_error);
```

- `new(code, message)`：创建基础错误
- `with_data(serializable)`：附加业务数据（进入响应 `data` 字段）
- `with_source(err)`：关联底层错误（仅服务端日志，不返回前端）
- 任意 `std::error::Error` 经 `From` 自动转换为 500"服务器内部错误"

### 7. 扩展点

#### TcpListenerFactory（监听器扩展）

默认实现直连 TCP 绑定。实现该 trait 并注册组件即可覆盖（默认实现带条件注册，用户提供实现时自动退位）：

```rust
#[simple_starter_core::component]
pub struct TlsListenerFactory;

#[simple_starter_core::injectable]
#[async_trait::async_trait]
impl TcpListenerFactory for TlsListenerFactory {
    async fn bind(&self, host: &str, port: u16) -> simple_starter_core::anyhow::Result<TcpListener> {
        // 在此构建 TLS / UDS 监听器
        todo!()
    }
}
```

#### WebExtensionRegistry（路由/中间件扩展）

供其他插件在 `assemble` 阶段通过应用上下文获取并注册扩展：

```rust
async fn assemble(&mut self, ctx: &mut Application) -> anyhow::Result<()> {
    ctx.get_extension_mut::<WebExtensionRegistry>()?
        .add_middleware(|router| router.layer(CompressionLayer::new()));
    Ok(())
}
```

## 三、组合使用示例

以下示例串联组件、REST 控制器与统一响应（`UserContext` 由 security 中间件注入）：

```rust
use simple_starter_core::{component, inject, Application};
use simple_starter_web::{json_response_wrap, post_mapping, rest_controller, JsonResponse, WebPlugin};
use simple_starter_web::axum::extract;
use std::sync::Arc;

#[component]
struct StudentService;
impl StudentService {
    async fn add(&self, name: &str) -> anyhow::Result<()> { Ok(()) }
}

#[component]
struct StudentController {
    #[inject]
    student_service: Arc<StudentService>,
}

#[rest_controller("/api")]
impl StudentController {
    #[post_mapping("/student/add")]
    async fn add_student(&self, extract::Json(student): extract::Json<StudentDto>) -> JsonResponse {
        json_response_wrap!(function_name = "添加学生", {
            self.student_service.add(&student.name).await?;
            Ok(())
        })
    }
}

fn main() {
    Application::new()
        .register_plugin(WebPlugin::new())
        .add_default_config(toml::toml! {
            [web]
            base_path = "/api"
        })
        .run();
}
```

## 四、配置项（`[web]` 节点）

```toml
[web]
port = 8080                 # 监听端口
binding = "0.0.0.0"         # 绑定地址
base_path = "/api"          # 全局路径前缀（可选，所有路由挂载其下）
log_include_headers = false # 是否在 Trace 日志中记录请求/响应头
```
