//! # simple_starter_core
//!
//! 应用核心库，提供插件系统、配置管理、组件容器、定时任务等基础设施。

// 模块声明
mod core {
    pub mod app_component;
    pub mod app_config;
    pub mod app_error;
    pub mod app_job;
    pub mod app_plugin;
}

mod loaders {
    pub mod component_loader;
    pub mod config_loader;
}

mod utils {
    pub mod app_core_util;
}

mod application;
mod global_state;

// 公共 API 导出
pub use anyhow; // 用于错误处理
pub use application::Application;
pub use core::app_component::ComponentFactory;
pub use core::app_error::AppCoreError;
pub use core::app_job::CronJob;
pub use core::app_plugin::Plugin;
pub use inventory::submit; // 用于宏注册
pub use tracing; // 方便用户直接使用 tracing
pub use utils::app_core_util::AppCoreUtil;
