//! # rust_starter_macros
//!
//! 本 crate 提供一组用于自动注册组件、依赖注入、定时任务和 Web 路由的 **过程宏（proc-macro）**，
//! 主要包含以下属性宏：
//! - `#[auto_component]`：标记组件工厂函数，自动注册到全局容器；
//! - `#[auto_inject]`：为结构体生成依赖获取方法；
//! - `#[cron_job]`：将异步函数注册为 cron 定时任务；
//! - HTTP 方法宏（`#[get]`, `#[post]` 等）：声明式注册 axum 路由；
//! - `#[json_response]`：自动将 handler 返回值包装为 JSON 响应。

// 引入标准库 proc_macro 支持
use proc_macro::TokenStream;

// 引入内部模块
mod auto_component_macro;
mod auto_inject_macro;
mod cron_job_macro;
mod route_macro;
mod json_response_macro;

// 公共 API：导出所有过程宏

/// 将一个 async 函数注册为 cron 定时任务。
///
/// # 用法示例
/// ```rust,ignore
/// #[cron_job("0 0 * * *")] // 每天午夜执行
/// async fn daily_cleanup() {
///     println!("清理完成");
/// }
/// ```
#[proc_macro_attribute]
pub fn cron_job(args: TokenStream, item: TokenStream) -> TokenStream {
    cron_job_macro::cron_job_macro(args, item)
}

/// 将一个无参函数注册为组件工厂，其返回值会被自动注册到组件容器中。
///
/// 可选指定 `name = "..."` 自定义组件名，默认使用返回类型的名称。
///
/// # 用法示例
/// ```rust,ignore
/// #[auto_component]
/// fn create_logger() -> Logger {
///     Logger::new()
/// }
///
/// #[auto_component(name = "main_db")]
/// fn db_factory() -> Database {
///     Database::connect("prod")
/// }
/// ```
#[proc_macro_attribute]
pub fn auto_component(args: TokenStream, item: TokenStream) -> TokenStream {
    auto_component_macro::auto_component_macro(args, item)
}

/// 为结构体生成依赖注入方法。
///
/// 支持两种注入方式：
/// - `types(MyService)`：生成 `get_my_services()` 方法，返回该类型的所有组件（Vec）
/// - `names(("cache", Cache))`：生成 `get_cache()` 方法，按名称获取特定组件
///
/// # 用法示例
/// ```rust,ignore
/// #[auto_inject(
///     types(Logger, Database),
///     names(("auth_service", AuthService))
/// )]
/// struct AppContext {}
///
/// // 自动生成：
/// // - get_loggers() -> Option<Vec<Arc<RwLock<Logger>>>>
/// // - get_databases() -> Option<Vec<Arc<RwLock<Database>>>>
/// // - get_auth_service() -> Option<Arc<RwLock<AuthService>>>
/// ```
#[proc_macro_attribute]
pub fn auto_inject(args: TokenStream, item: TokenStream) -> TokenStream {
    auto_inject_macro::auto_inject_macro(args, item)
}

// --- Web 路由宏 ---

/// 注册一个 GET 路由。
///
/// 支持两种参数形式：
/// - 位置参数：`#[get("/hello")]`
/// - 命名参数：`#[get(path = "/api", state = AppState::new())]`
///
/// 生成的路由会通过 `inventory::submit!` 自动收集，用于应用启动时注册。
#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro::route_macro("get", args, input)
}

/// 注册一个 POST 路由（用法同 `get`）。
#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro::route_macro("post", args, input)
}

/// 注册一个 PUT 路由（用法同 `get`）。
#[proc_macro_attribute]
pub fn put(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro::route_macro("put", args, input)
}

/// 注册一个 DELETE 路由（用法同 `get`）。
#[proc_macro_attribute]
pub fn delete(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro::route_macro("delete", args, input)
}

/// 自动将 handler 的返回值包装为 `Json<T>` 响应。
///
/// 要求：
/// - 函数必须是 `async`
/// - 必须有显式返回类型（不能是 `-> ()`）
///
/// # 用法示例
/// ```rust,ignore
/// #[json_response]
/// async fn user_info() -> UserInfo {
///     UserInfo { id: 1, name: "Alice".into() }
/// }
/// // 实际生成：-> Json<UserInfo>
/// ```
#[proc_macro_attribute]
pub fn json_response(args: TokenStream, input: TokenStream) -> TokenStream {
    json_response_macro::json_response_macro(args, input)
}
