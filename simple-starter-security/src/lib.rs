//! Simple Starter Security 模块。
//!
//! 提供编译期资源收集、运行时白名单、用户认证、权限校验等安全能力。

mod auth_middleware;
mod resource;
mod security_plugin;
mod whitelist;

// 公共导出
pub use auth_middleware::{
    PermissionChecker, SecurityError, SecurityErrorHandler, UserContext, UserInfoProvider,
};
pub use inventory::submit;
pub use resource::ResourceEntry;
pub use security_plugin::{BasePathProvider, SecurityPlugin};
