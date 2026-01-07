//! Web 插件核心库入口。
//!
//! 本模块整合了 Web 服务所需的核心组件：
//! - 路由宏（通过 `route_macro` 实现）
//! - 路由注册机制（`RouteFactory`）
//! - Web 插件主逻辑（`WebPlugin`）
//! - 配置结构（`WebConfig`）
//!
//! 同时重新导出关键类型，供外部 crate 使用。

// 内部模块组织
mod web_plugin;
mod config {
    pub mod web_config;
}
mod router {
    pub mod router_factory;
}

mod support {
    pub mod app_web_error;
    pub mod json_response;
}

// 重新导出关键类型，方便用户使用
pub use web_plugin::WebPlugin;       // Web 服务插件实现
pub use axum;                        // 导出 axum，避免用户重复依赖
pub use router::router_factory::RouteFactory; // 路由工厂类型，用于自动注册
pub use inventory::submit;           // 用于在编译期收集路由
pub use support::app_web_error::SimpleAppWebError;
pub use support::json_response::JsonResponse;
pub use support::json_response::process_data;