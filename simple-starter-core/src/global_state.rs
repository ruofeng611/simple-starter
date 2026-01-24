use crate::core::app_component::ComponentProcessor;
use crate::core::app_types::ComponentKey;
use dashmap::DashMap;
use std::sync::{LazyLock, OnceLock};

/// 全局配置存储
///
/// 使用 `OnceLock` 保证只能被写入一次（通常在启动加载配置时）。
/// 存储的是解析后的 TOML Value，后续可以通过 `AppCoreUtil` 进行路径查询。
pub(crate) static GLOBAL_CONFIG: OnceLock<toml::Value> = OnceLock::new();

/// 组件仓库
///
/// 存储所有注册的组件实例。
/// - Key: `(TypeId, String)` 组成的唯一标识。
/// - Value: `Box<dyn ComponentProcessor>` 组件处理器对象。
/// - 使用 `DashMap` 提供高并发的读写能力，无需手动加锁。
/// - 使用 `LazyLock` 确保首次访问时才进行初始化。
pub(crate) static COMPONENT_REPOSITORY: LazyLock<
    DashMap<ComponentKey, Box<dyn ComponentProcessor>>,
> = LazyLock::new(DashMap::new);
