mod core {
    pub(crate) mod component_macro;
    pub(crate) mod configuration_macro;
    pub(crate) mod cron_job_macro;
    pub(crate) mod event_listener_macro;
    pub(crate) mod injectable_macro;
    pub(crate) mod primary_macro;
    pub(crate) mod provider_macro;
}

mod web {
    pub(crate) mod json_response_macro;
    pub(crate) mod rest_controller_macro;
    pub(crate) mod route_macro;
}

mod security {
    pub(crate) mod security_controller_macro;
    pub(crate) mod security_macro;
    pub(crate) mod security_utils;
}

mod utils {
    pub(crate) mod macro_build_util;
}

use proc_macro::TokenStream;

// =============================================================================
// Core 模块宏
// =============================================================================

/// 定义一个组件。
///
/// 该宏将结构体注册到依赖注入容器中。
/// 支持 `init_method` 和 `destroy_method` 生命周期回调。
///
/// 用法：
/// ```rust
/// #[component]
/// pub struct UserService {
///     #[inject]
///     user_repository: Arc<UserRepository>,
/// }
/// ```
#[proc_macro_attribute]
pub fn component(args: TokenStream, item: TokenStream) -> TokenStream {
    core::component_macro::component_macro(args, item)
}

/// 标记一个 trait 实现可用于 trait object 注入。
///
/// 将 `impl Trait for Type` 注册为 trait 的可注入实现，
/// 允许其他组件通过 `Arc<dyn Trait>` 或 `Vec<Arc<dyn Trait>>` 注入该实现。
///
/// 用法：
/// ```rust
/// #[injectable]
/// impl Greeter for HelloGreeter {
///     fn greet(&self) -> String {
///         "Hello!".into()
///     }
/// }
/// ```
///
/// 注意：必须搭配 `#[component]` 标注的结构体使用。
/// `#[injectable]` 仅注册 trait→type 映射，不注册组件本身。
#[proc_macro_attribute]
pub fn injectable(args: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = match syn::parse2::<syn::ItemImpl>(item.into()) {
        Ok(impl_block) => impl_block,
        Err(e) => return e.to_compile_error().into(),
    };
    core::injectable_macro::injectable_on_impl(args, item_impl)
        .unwrap_or_else(|e| e.to_compile_error().into())
}

/// 定义一个组件提供者（Provider）。
///
/// 将函数转换为组件工厂，用于创建无法直接修改源码的第三方类型实例。
/// 返回值必须为 `Result<T>` 或 `anyhow::Result<T>`，宏会自动提取内部类型 `T` 作为组件类型。
///
/// 用法：
/// ```rust
/// #[provider]
/// pub fn create_http_client() -> anyhow::Result<reqwest::Client> {
///     Ok(reqwest::Client::new())
/// }
/// ```
#[proc_macro_attribute]
pub fn provider(args: TokenStream, item: TokenStream) -> TokenStream {
    core::provider_macro::provider_macro(args, item)
}

/// 标记首要（primary）实例。
///
/// 必须与 `#[provider]` 一起标注在同一函数上，声明该函数返回值类型的首要实例：
/// 当框架按类型获取组件时优先返回它（见 `AppCoreUtil::get_primary_component`）。
/// 由于存在 primary 通常意味着同类型有多个实例，因此必须显式指定实例名，
/// 且该名字必须与 `#[provider]` 注册的组件名一致。
///
/// 用法：
/// ```rust
/// #[provider(name = "mainRedis")]
/// #[primary(name = "mainRedis")]
/// pub fn main_redis() -> anyhow::Result<redis::Client> {
///     Ok(redis::Client::open("redis://main")?)
/// }
/// ```
///
/// 也支持位置参数简写：`#[primary("mainRedis")]`。
#[proc_macro_attribute]
pub fn primary(args: TokenStream, item: TokenStream) -> TokenStream {
    core::primary_macro::primary_macro(args, item)
}

/// 定义一个配置组件。
///
/// 该宏将结构体注册为配置组件，字段自动绑定 TOML 配置路径。
///
/// 用法：
/// ```rust
/// #[configuration(prefix = "app")]
/// pub struct AppConfig {
///     pub name: String,
///     pub port: u16,
/// }
/// ```
#[proc_macro_attribute]
pub fn configuration(args: TokenStream, item: TokenStream) -> TokenStream {
    core::configuration_macro::configuration_macro(args, item)
}

/// 标记依赖注入字段。
///
/// 注意：实际逻辑在 `component` 或 `provider` 宏中处理，此宏仅用于通过语法检查。
///
/// 用法：
/// ```rust
/// #[component]
/// pub struct UserService {
///     #[inject]
///     user_repository: Arc<UserRepository>,
/// }
/// ```
#[proc_macro_attribute]
pub fn inject(_args: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// 定义定时任务。
///
/// 将 async 函数注册为 CronJob。
///
/// 用法：
/// ```rust
/// #[cron_job("0 0 * * * *")]
/// async fn daily_cleanup() {
///     // 每小时执行一次
/// }
/// ```
#[proc_macro_attribute]
pub fn cron_job(args: TokenStream, item: TokenStream) -> TokenStream {
    core::cron_job_macro::cron_job_macro(args, item)
}

/// 注册事件监听器。
///
/// 作用在 `impl EventListener<E> for Type` 块上，将实现组件注册为该事件类型的监听器：
/// 发布器（默认 `DefaultEventPublisher`）在 init 阶段自动收集，事件发布时同步广播。
/// 同时生成 trait 实现映射，`#[inject] Vec<Arc<dyn EventListener<E>>>` 可正常注入。
///
/// 用法：
/// ```rust
/// #[component]
/// pub struct UserService { ... }
///
/// #[event_listener]
/// #[async_trait::async_trait]
/// impl EventListener<UserLoginEvent> for UserService {
///     async fn on_event(&self, event: &UserLoginEvent) -> anyhow::Result<()> {
///         // 处理登录事件
///         Ok(())
///     }
/// }
/// ```
///
/// 注意：
/// - 必须搭配 `#[component]` 标注的结构体使用（监听器需是已注册组件）；
///   发布器 init 阶段收集全部监听器组件，监听器由组件仓库强持有至应用结束。
#[proc_macro_attribute]
pub fn event_listener(args: TokenStream, item: TokenStream) -> TokenStream {
    let item_impl = match syn::parse2::<syn::ItemImpl>(item.into()) {
        Ok(impl_block) => impl_block,
        Err(e) => return e.to_compile_error().into(),
    };
    core::event_listener_macro::event_listener_on_impl(args, item_impl)
        .unwrap_or_else(|e| e.to_compile_error().into())
}

// =============================================================================
// Web 模块宏
// =============================================================================

/// RestController 宏：用于在 impl 块上标记基础路径，并自动为带 mapping 宏的方法生成 Axum 路由。
///
/// 所有方法的返回值自动用 `Json<T>` 包裹。
/// mapping 宏（get_mapping 等）仅起标记作用，不会被移除。
///
/// 用法：
/// ```rust
/// #[component]
/// pub struct TestController {
///     #[inject]
///     student_service: Arc<StudentService>,
/// }
///
/// #[rest_controller("/test")]
/// impl TestController {
///     #[get_mapping("/student/{id}")]
///     pub async fn get_student(&self, Path(id): Path<i64>) -> StudentVO {
///         // ...
///     }
/// }
/// ```
#[proc_macro_attribute]
pub fn rest_controller(args: TokenStream, item: TokenStream) -> TokenStream {
    web::rest_controller_macro::rest_controller_macro(args, item)
}

/// 定义 GET 路由。
///
/// 用于自由函数上，配合 `RouteFactory` 注册 GET 路由。
///
/// 用法：
/// ```rust
/// #[get("/users")]
/// async fn list_users() -> Json<Vec<User>> {
///     Json(vec![])
/// }
/// ```
#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    web::route_macro::route_macro("get", args, input)
}

/// 定义 POST 路由。
///
/// 用于自由函数上，配合 `RouteFactory` 注册 POST 路由。
///
/// 用法：
/// ```rust
/// #[post("/users")]
/// async fn create_user(user: Json<CreateUser>) -> Json<User> {
///     Json(User::new())
/// }
/// ```
#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    web::route_macro::route_macro("post", args, input)
}

/// 定义 PUT 路由。
///
/// 用于自由函数上，配合 `RouteFactory` 注册 PUT 路由。
///
/// 用法：
/// ```rust
/// #[put("/users/:id")]
/// async fn update_user(id: Path<i64>, user: Json<UpdateUser>) -> Json<User> {
///     Json(User::new())
/// }
/// ```
#[proc_macro_attribute]
pub fn put(args: TokenStream, input: TokenStream) -> TokenStream {
    web::route_macro::route_macro("put", args, input)
}

/// 定义 DELETE 路由。
///
/// 用于自由函数上，配合 `RouteFactory` 注册 DELETE 路由。
///
/// 用法：
/// ```rust
/// #[delete("/users/:id")]
/// async fn delete_user(id: Path<i64>) -> &'static str {
///     "deleted"
/// }
/// ```
#[proc_macro_attribute]
pub fn delete(args: TokenStream, input: TokenStream) -> TokenStream {
    web::route_macro::route_macro("delete", args, input)
}

// --- Controller Mapping 宏 ---
// 这些宏用于标记 controller 中的方法，本身不生成代码，只作为标记供 rest_controller 宏解析。
// rest_controller 宏扫描到这些标记后不会移除它们。

/// GET 路由标记宏。
///
/// 作用在 `impl` 块的方法上，供 `#[rest_controller]` 扫描识别。
///
/// 用法：
/// ```rust
/// #[get_mapping("/student/{id}")]
/// pub async fn get_student(&self, id: i64) -> StudentVO { }
/// ```
#[proc_macro_attribute]
pub fn get_mapping(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// POST 路由标记宏。
///
/// 作用在 `impl` 块的方法上，供 `#[rest_controller]` 扫描识别。
///
/// 用法：
/// ```rust
/// #[post_mapping("/students")]
/// pub async fn create_student(&self, student: StudentDTO) -> StudentVO { }
/// ```
#[proc_macro_attribute]
pub fn post_mapping(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// PUT 路由标记宏。
///
/// 作用在 `impl` 块的方法上，供 `#[rest_controller]` 扫描识别。
///
/// 用法：
/// ```rust
/// #[put_mapping("/students/{id}")]
/// pub async fn update_student(&self, id: i64, student: StudentDTO) -> StudentVO { }
/// ```
#[proc_macro_attribute]
pub fn put_mapping(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// DELETE 路由标记宏。
///
/// 作用在 `impl` 块的方法上，供 `#[rest_controller]` 扫描识别。
///
/// 用法：
/// ```rust
/// #[delete_mapping("/students/{id}")]
/// pub async fn delete_student(&self, id: i64) -> &'static str { }
/// ```
#[proc_macro_attribute]
pub fn delete_mapping(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// 简化 JSON 响应处理。
///
/// 自动将 handler 的返回值 `T` 包装为 `Json<T>`。
///
/// 用法：
/// ```rust
/// #[json_response]
/// async fn get_user() -> User {
///     User::new()
/// }
/// ```
#[proc_macro_attribute]
pub fn json_response(args: TokenStream, input: TokenStream) -> TokenStream {
    web::json_response_macro::json_response_macro(args, input)
}

// =============================================================================
// Security 模块宏
// =============================================================================

/// Security 资源标记宏。
///
/// 用于编译期收集自由函数 Web 接口的资源信息（路径、资源标识、资源名称、模块信息）。
///
/// 用法：
/// ```rust
/// #[security(resource_id = "user:list", resource_name = "查询用户")]
/// #[get("/users")]
/// async fn list_users() { }
/// ```
#[proc_macro_attribute]
pub fn security(args: TokenStream, input: TokenStream) -> TokenStream {
    security::security_macro::security_macro(args, input)
}

/// Security Controller 宏。
///
/// 作用在 `impl` 块上（必须在 `#[rest_controller]` 外层），为标记了 `#[security_resource]`
/// 的方法批量注册安全资源信息。
///
/// 用法：
/// ```rust
/// #[security_controller(module_id = "user", module_name = "用户管理")]
/// #[rest_controller("/api/users")]
/// impl UserController {
///     #[get_mapping("/:id")]
///     #[security_resource(resource_id = "user:detail", resource_name = "查询用户详情")]
///     async fn get_user(&self, id: i64) -> Json<User> { }
/// }
/// ```
#[proc_macro_attribute]
pub fn security_controller(args: TokenStream, input: TokenStream) -> TokenStream {
    security::security_controller_macro::security_controller_macro(args, input)
}

/// Security Resource 标记宏。
///
/// 作用在 `impl` 块的方法上，配合 `#[security_controller]` 使用。
/// 本身不生成代码，仅作为标记供 `#[security_controller]` 读取资源信息。
///
/// 用法：
/// ```rust
/// #[get_mapping("/:id")]
/// #[security_resource(resource_id = "user:detail")]
/// async fn get_user(&self, id: i64) -> Json<User> { }
/// ```
#[proc_macro_attribute]
pub fn security_resource(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
