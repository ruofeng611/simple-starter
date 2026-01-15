//! # 全局状态管理模块
//!
//! 本模块定义了整个应用共享的全局状态，包括：
//! - `GLOBAL_CONFIG`：加载后的最终配置（TOML 格式）
//! - `COMPONENT_REPOSITORY`：组件注册表，用于按类型和名称存储组件实例

use dashmap::DashMap;
use std::any::{Any, TypeId};
use std::sync::{Arc, LazyLock, OnceLock};
use toml::Value;

/// 全局配置单例（OnceLock 保证只初始化一次）
///
/// 通过 `config_loader::global_config_load` 初始化，
/// 可通过 `AppCoreUtil::get_config_value_by_path` 读取。
pub(crate) static GLOBAL_CONFIG: OnceLock<Value> = OnceLock::new();

/// 组件键类型：(类型 ID, 名称)
///
/// 用于在组件仓库中唯一标识一个组件。
pub(crate) type ComponentKey = (TypeId, String);

/// 全局组件仓库（惰性初始化 + 并发安全读写）
///
/// 存储所有托管的组件实例。来源包括：
/// 1. `#[auto_component]` 宏自动扫描注册的组件。
/// 2. 代码中显式调用 `AppCoreUtil::register_component` 注册的组件。
///
/// 使用 `DashMap` 提供高并发读写能力。
pub(crate) static COMPONENT_REPOSITORY: LazyLock<
    DashMap<ComponentKey, Arc<dyn Any + Send + Sync>>,
> = LazyLock::new(DashMap::new);
