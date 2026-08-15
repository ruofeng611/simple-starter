//! Simple Starter Security 模块。
//!
//! 提供编译期资源收集、运行时白名单、用户认证、权限校验等安全能力。
//!
//! # 使用方式
//!
//! ```ignore
//! simple_starter_core::Application::new()
//!     .register_plugin(WebPlugin::new())
//!     .register_plugin(SecurityPlugin::new())
//!     .run();
//! ```

mod auth_middleware;
mod resource;
mod security_plugin;
mod whitelist;

// 公共导出
pub use auth_middleware::{
    PermissionChecker, SecurityError, SecurityErrorHandler, UserContext, UserInfoProvider,
};
// Security 宏重导出（依赖方无需直接依赖 simple-starter-macro）
pub use simple_starter_macro::{security, security_controller, security_resource};
pub use resource::ResourceEntry;
pub use security_plugin::{BasePathProvider, SecurityPlugin};
