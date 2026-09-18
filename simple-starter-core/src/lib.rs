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
//!
//! ## 模块结构
//!
//! - `model`：领域模型层（组件、容器、插件、上下文、条件、任务、扩展存储）
//! - `loaders`：装配过程层（配置加载等启动期流程；组件装配随组件模型聚合）
//! - `utils`：工具层（公开工具 `core_util` 与内部工具 `inner_util`）
//! - `application`：应用编排层（Application 启动/关闭流程）
//! - `event`：事件系统

// 允许本 crate 内部宏展开代码经 `::simple_starter_core::...` 路径解析自身
// （crate 名字不在自身 extern prelude 中，需显式声明别名）
extern crate self as simple_starter_core;

// === 内部模块定义 ===

/// 领域模型层（组件/插件/上下文/条件/任务/扩展存储的定义与自身行为）
mod model {
    pub(crate) mod component;
    pub(crate) mod condition;
    pub(crate) mod context;
    pub(crate) mod extensions;
    pub(crate) mod job;
    pub(crate) mod plugin;
}

/// 装配过程层（配置加载等启动期流程）
mod loaders {
    pub(crate) mod config_loader;
}

/// 工具层（公开工具与内部工具）
mod utils {
    pub(crate) mod assembly_guard;
    pub(crate) mod core_util;
    pub(crate) mod freeze_cell;
    pub(crate) mod inner_util;
}

/// 应用程序主入口逻辑（应用编排层）
mod application;

/// 事件系统（Spring 风格事件发布/监听）
mod event {
    pub mod app_event;
    pub mod event_listener;
    pub mod event_publisher;
}

// === 公共导出 (Public API) ===
// 重新导出常用 crate 和核心结构，方便下游用户直接使用，无需并在 Cargo.toml 中重复引入基础库。

use std::future::Future;
use std::pin::Pin;

/// 异步 Future 的装箱类型
///
/// 用于在 Trait 对象或动态分发中返回异步任务。
/// - `Pin<Box<...>>`: 也就是堆上的固定位置 Future。
/// - `Send`: 允许跨线程移动。
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

// 1. 基础依赖重导出
pub use anyhow; // 用于统一的 Result<T, anyhow::Error> 错误处理
pub use inventory::submit; // 用于 submit! 宏进行组件/路由注册（全部宏展开代码统一经此路径）
pub use toml; // 配置值类型（宏展开代码与用户配置访问使用）
pub use tracing; // 用于日志记录 (info!, error!, debug! 等)

// 2. 核心宏重导出（依赖方无需直接依赖 simple-starter-macro）
pub use simple_starter_macro::{
    component, configuration, cron_job, event_listener, inject, injectable, lifecycle, provider,
};

// 3. 核心入口与工具
pub use application::Application; // 应用程序启动入口

// 4. 组件与插件系统
pub use model::component::ComponentContainer; // 组件容器（组件查询）
pub use model::component::ComponentProcessorFactory; // 组件工厂（宏生成使用）
pub use model::component::ComponentWrapper; // 组件包装器
pub use model::component::{CreateFn, DestroyFn, InitFn}; // 组件生命周期函数签名（宏生成使用）
pub use model::component::TraitObjAccessorFn; // trait object 访问器签名（宏生成使用）
pub use model::context::AppContext; // 应用上下文（插件就绪/收尾期与钩子的协作面）
pub use model::context::global_context::{app_config, app_container}; // 全局上下文快照（无上下文传播场景的只读访问点）
pub use model::extensions::Extensions; // 扩展存储（插件 assemble 阶段的唯一协作面）
pub use model::plugin::Plugin; // 插件 Trait

// 5. 配置查询（全局配置由 AppContext 持有，经 AppContext / create 回调分发）
pub use utils::core_util::{get_config_to_struct, get_config_value_by_path}; // 点分路径配置查询与反序列化

// 6. Trait object 注入支持
pub use model::component::Injectable; // 可注入 trait 的 super_trait
pub use model::component::PrimaryRegistration; // primary 实例注册（宏生成使用）
pub use model::component::TraitImplRegistration; // trait 实现注册（宏生成使用）
pub use model::component::TraitObjectEntry; // trait object 缓存条目（宏生成使用）

// 7. 容器级生命周期（全部就绪 / 销毁之前，宏生成使用）
pub use model::component::{ComponentLifecycle, LifecycleRegistration};

// 8. 任务系统
pub use model::job::CronJob; // 定时任务结构

// 9. 条件注册支持
pub use model::condition::{ComponentCondition, ConditionContext}; // 组件条件声明与评估上下文

// 10. 类型定义与错误扩展
pub use model::component::ComponentError; // 组件查询错误
pub use utils::core_util::LogExpectExt; // 扩展 Result/Option 的 log_expect 方法
pub use utils::core_util::TomlConfigError; // 配置读取错误

// 11. 事件系统
pub use event::app_event::{AppEvent, EventListenerRegistration}; // 事件标记 trait 与监听器注册（宏生成使用）
pub use event::event_listener::{AnyEventListener, EventListener, TypedListenerAdapter}; // 监听器与适配器
pub use event::event_publisher::{EventPublisher, EventPublisherExt}; // 发布器 trait 与类型化便捷扩展
