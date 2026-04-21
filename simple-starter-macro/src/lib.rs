mod component_macro;
mod rest_controller_macro;
mod cron_job_macro;
mod inject_macro;
mod json_response_macro;
mod provider_macro;
mod route_macro;
mod configuration_macro;

mod utils {
    pub(crate) mod macro_build_util;
}

use proc_macro::TokenStream;

/// 定义一个组件。
///
/// 该宏将结构体注册到依赖注入容器中。
/// 支持 `init_method` 和 `destroy_method` 生命周期回调。
#[proc_macro_attribute]
pub fn component(args: TokenStream, item: TokenStream) -> TokenStream {
    component_macro::component_macro(args, item)
}

/// 定义一个组件提供者（Provider）。
///
/// 将函数转换为组件工厂，用于创建无法直接修改源码的第三方类型实例。
#[proc_macro_attribute]
pub fn provider(args: TokenStream, item: TokenStream) -> TokenStream {
    provider_macro::provider_macro(args, item)
}

/// 定义一个配置组件
///
/// 该宏将结构体注册为配置组件。
#[proc_macro_attribute]
pub fn configuration(args: TokenStream, item: TokenStream) -> TokenStream {
    configuration_macro::configuration_macro(args, item)
}

/// 标记依赖注入字段。
///
/// 注意：实际逻辑在 `component` 或 `provider` 宏中处理，此宏仅用于通过语法检查。
#[proc_macro_attribute]
pub fn inject(_args: TokenStream, item: TokenStream) -> TokenStream {
    inject_macro::inject_macro(_args, item)
}

/// 定义定时任务。
///
/// 将 async 函数注册为 CronJob。
/// 用法: `#[cron_job("0 * * * * *")]`
#[proc_macro_attribute]
pub fn cron_job(args: TokenStream, item: TokenStream) -> TokenStream {
    cron_job_macro::cron_job_macro(args, item)
}

// --- Web 路由宏 ---

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
    rest_controller_macro::rest_controller_macro(args, item)
}

#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro::route_macro("get", args, input)
}

#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro::route_macro("post", args, input)
}

#[proc_macro_attribute]
pub fn put(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro::route_macro("put", args, input)
}

#[proc_macro_attribute]
pub fn delete(args: TokenStream, input: TokenStream) -> TokenStream {
    route_macro::route_macro("delete", args, input)
}

// --- Controller Mapping 宏 ---
// 这些宏用于标记 controller 中的方法，本身不生成代码，只作为标记供 rest_controller 宏解析。
// rest_controller 宏扫描到这些标记后不会移除它们。

#[proc_macro_attribute]
pub fn get_mapping(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn post_mapping(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn put_mapping(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn delete_mapping(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// 简化 JSON 响应处理。
///
/// 自动将 handler 的返回值 `T` 包装为 `Json<T>`。
#[proc_macro_attribute]
pub fn json_response(args: TokenStream, input: TokenStream) -> TokenStream {
    json_response_macro::json_response_macro(args, input)
}