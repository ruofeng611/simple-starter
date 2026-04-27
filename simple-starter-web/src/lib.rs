//! Web 插件核心库入口。
//!
//! 本模块整合了 Web 服务所需的核心组件，对外提供统一的 API。

// 内部模块定义
mod web_plugin;
mod server_builder;
mod web_extension;

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

// === 公共导出 ===

// 核心插件
pub use web_plugin::WebPlugin;

// 重新导出 axum，方便下游使用，保持版本一致
pub use axum;

// 路由注册相关
pub use router::router_factory::RouteFactory;
pub use inventory::submit;

// 扩展注册表（供其他插件注册中间件、路由修改器、自定义监听器）
pub use web_extension::WebExtensionRegistry;

// 错误处理与响应封装
pub use support::app_web_error::SimpleAppWebError;
pub use support::json_response::JsonResponse;
pub use support::json_response::process_data;