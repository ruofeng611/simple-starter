mod component_macro;
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

/// 简化 JSON 响应处理。
///
/// 自动将 handler 的返回值 `T` 包装为 `Json<T>`。
#[proc_macro_attribute]
pub fn json_response(args: TokenStream, input: TokenStream) -> TokenStream {
    json_response_macro::json_response_macro(args, input)
}