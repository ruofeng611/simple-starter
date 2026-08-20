use crate::core::app_component::{ComponentProcessor, TraitObjectEntry};
use crate::core::app_types::ComponentKey;
use dashmap::DashMap;
use std::any::TypeId;
use std::sync::{LazyLock, OnceLock};

/// 全局配置存储
///
/// 使用 `OnceLock` 保证只能被写入一次（通常在启动加载配置时）。
/// 存储的是解析后的 TOML Value，后续可以通过 `AppCoreUtil` 进行路径查询。
pub(crate) static GLOBAL_CONFIG: OnceLock<toml::Value> = OnceLock::new();

/// 组件仓库
///
/// 存储所有注册的组件实例。
/// - Key: 组件名（全局唯一，含跨具体类型）。
/// - Value: `Box<dyn ComponentProcessor>` 组件处理器对象（类型信息由 `type_id()` 提供）。
/// - 使用 `DashMap` 提供高并发的读写能力，无需手动加锁。
/// - 使用 `LazyLock` 确保首次访问时才进行初始化。
pub(crate) static COMPONENT_REPOSITORY: LazyLock<
    DashMap<ComponentKey, Box<dyn ComponentProcessor>>,
> = LazyLock::new(DashMap::new);

/// Trait object 缓存
///
/// - Key: `(trait_type_id, 组件实例名)`
/// - Value: `TraitObjectEntry` — 类型擦除的 trait object + 还原用 vtable（coercion 时记录）
///   在组件 create 后立即填充，inject 时 clone 取出并按记录 vtable 拼回 `Arc<dyn Trait>`。
pub(crate) static TRAIT_OBJ_CACHE: LazyLock<
    DashMap<(TypeId, String), TraitObjectEntry>
> = LazyLock::new(DashMap::new);

/// 具体类型 → 该类型所有已创建实例名 的索引
///
/// 在 `populate_trait_obj_cache`（每个组件 create 后）填充，
/// 供 `get_component` 按类型兜底查找使用，避免运行时扫描仓库。
pub(crate) static TYPE_INSTANCE_NAMES: LazyLock<
    DashMap<TypeId, Vec<String>>
> = LazyLock::new(DashMap::new);

/// trait_type_id → 该 trait 所有已创建实现组件的实例名 的索引
///
/// 在 `populate_trait_obj_cache`（每个组件 create 后）同步填充，
/// 供 `get_component_by_trait` / `get_components_by_trait` 使用。
/// 拓扑排序保证依赖组件先于依赖者创建，依赖者 create 阶段访问时索引必已填充。
pub(crate) static TRAIT_INSTANCE_NAMES: LazyLock<
    DashMap<TypeId, Vec<String>>
> = LazyLock::new(DashMap::new);

/// 具体类型 → 该类型的 primary（首要）实例名 的索引
///
/// 启动注册期由 inventory `PrimaryRegistration` 构建（条件过滤后，
/// 并校验名字对应组件存在、同类型唯一），供 `get_primary_component`
/// 按类型取首要实例使用。
pub(crate) static PRIMARY_BY_TYPE: LazyLock<
    DashMap<TypeId, String>
> = LazyLock::new(DashMap::new);
