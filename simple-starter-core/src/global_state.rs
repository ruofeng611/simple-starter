use crate::core::app_component::{ComponentProcessor, Injectable};
use crate::core::app_types::ComponentKey;
use dashmap::DashMap;
use std::any::TypeId;
use std::sync::{Arc, LazyLock, OnceLock};

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

/// Trait object 缓存
///
/// - Key: `(trait_type_id, 组件实例名)`
/// - Value: `Arc<dyn Injectable>` — 由 `Arc<dyn Trait>` 通过 upcasting coercion 转换而来
/// 在组件 create 后立即填充，inject 时 clone 取出。
pub(crate) static TRAIT_OBJ_CACHE: LazyLock<
    DashMap<(TypeId, String), Arc<dyn Injectable>>
> = LazyLock::new(DashMap::new);

/// trait_type_id → 所有实例名 的索引
///
/// 在 `populate_trait_obj_cache` 时同步填充，避免 `get_component_by_trait`
/// 在 create 阶段扫描 `COMPONENT_REPOSITORY` 造成死锁（create 持有写锁，
/// `iter()` 需要读同一个 shard）。
pub(crate) static INSTANCE_NAMES_BY_TRAIT: LazyLock<
    DashMap<TypeId, Vec<String>>
> = LazyLock::new(DashMap::new);
