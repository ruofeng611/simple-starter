//! # 应用核心框架
//!
//! 本 crate 提供了一个用于构建复杂 Rust 应用程序的框架，包含配置管理、插件系统、组件模型、任务调度等核心功能。
//! 旨在简化应用程序的启动流程、生命周期管理和依赖注入。
//!
//! ## 主要特性
//!
//! - **统一配置**: 通过 TOML 文件进行分层配置。支持默认配置、插件默认配置、用户主配置 (`application.toml`) 以及环境 Profile 配置 (`application-dev.toml`) 的自动合并。
//! - **插件系统**: 定义 `Plugin` trait，支持模块化扩展应用功能。框架会自动处理插件间的依赖关系（拓扑排序）和初始化顺序。
//! - **组件模型**: 基于 `inventory` 宏实现自动注册。支持组件的依赖注入、生命周期管理（创建 -> 初始化 -> 销毁）和基于依赖关系的启动顺序控制。
//! - **任务调度**: 集成 `tokio_cron_scheduler`，通过 `CronJob` 宏即可声明式地注册定时任务。
//! - **运行时管理**: 封装 Tokio 运行时，根据配置自动选择单线程或多线程运行时。支持 CLI 模式和接管主线程的 GUI 模式。
//! - **日志系统**: 集成 `tracing` 生态，支持控制台输出和文件轮转日志，配置灵活。

// 允许本 crate 内部宏展开代码经 `::simple_starter_core::...` 路径解析自身
// （crate 名字不在自身 extern prelude 中，需显式声明别名）
extern crate self as simple_starter_core;

// === 内部模块定义 ===

/// 核心领域模型定义（组件、配置、错误、任务、插件、类型）
mod core {
    pub(crate) mod app_component;
    pub(crate) mod app_config;
    pub(crate) mod app_error;
    pub(crate) mod app_job;
    pub(crate) mod app_plugin;
    pub(crate) mod app_types;
}

/// 加载器模块（负责组件和配置的加载逻辑）
mod loaders {
    pub(crate) mod component_loader;
    pub(crate) mod config_loader;
    pub(crate) mod component_condition;
}

/// 工具类模块（内部和外部通用的辅助函数）
mod utils {
    pub(crate) mod app_core_util;
    pub(crate) mod app_inner_util;
}

/// 应用程序主入口逻辑
mod application;

/// 扩展存储（AnyMap）
mod extensions;

/// 全局静态状态管理（配置单例、组件仓库）
mod global_state;

/// 事件系统（Spring 风格事件发布/监听）
mod event {
    pub mod app_event;
    pub mod event_listener;
    pub mod event_publisher;
}

// === 公共导出 (Public API) ===
// 重新导出常用 crate 和核心结构，方便下游用户直接使用，无需并在 Cargo.toml 中重复引入基础库。

// 1. 基础依赖重导出
pub use anyhow; // 用于统一的 Result<T, anyhow::Error> 错误处理
pub use inventory::submit; // 用于 submit! 宏进行组件/路由注册（全部宏展开代码统一经此路径）
pub use tracing; // 用于日志记录 (info!, error!, debug! 等)

// 1.5 核心宏重导出（依赖方无需直接依赖 simple-starter-macro）
pub use simple_starter_macro::{
    component, configuration, cron_job, event_listener, inject, injectable, primary, provider,
};

// 2. 核心入口与工具
pub use application::Application; // 应用程序启动入口
pub use utils::app_core_util::AppCoreUtil; // 获取配置、获取组件等核心工具

// 3. 组件与插件系统
pub use core::app_component::ComponentProcessorFactory; // 组件工厂（宏生成使用）
pub use core::app_component::ComponentWrapper; // 组件包装器
pub use core::app_plugin::Plugin; // 插件 Trait

// 4. Trait object 注入支持
pub use core::app_component::Injectable; // 可注入 trait 的 super_trait
pub use core::app_component::PrimaryRegistration; // primary 实例注册（宏生成使用）
pub use core::app_component::TraitImplRegistration; // trait 实现注册（宏生成使用）
pub use core::app_component::TraitObjectEntry; // trait object 缓存条目（宏生成使用）

// 5. 任务系统
pub use core::app_job::CronJob; // 定时任务结构

// 6. 条件注册支持
pub use loaders::component_condition::{ComponentCondition, ConditionContext}; // 组件条件声明与评估上下文

// 7. 类型定义与错误扩展
pub use core::app_error::LogExpectExt; // 扩展 Result/Option 的 log_expect 方法
pub use core::app_types::*; // 常用类型别名 (BoxFuture, ComponentKey 等)

// 8. 事件系统
pub use event::app_event::{AppEvent, EventListenerRegistration}; // 事件标记 trait 与监听器注册（宏生成使用）
pub use event::event_listener::{AnyEventListener, EventListener, TypedListenerAdapter}; // 监听器与适配器
pub use event::event_publisher::{DefaultEventPublisher, EventPublisher, EventPublisherExt}; // 发布器 trait 与默认实现