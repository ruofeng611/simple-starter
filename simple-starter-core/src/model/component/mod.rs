//! # 组件模型与组件容器（对标 Spring Bean + BeanFactory）
//!
//! 组件域的完整领域模型：组件生命周期抽象（bean 级三阶段 + 容器级批次周期）、
//! 编译期注册类型、组件容器及查询 API。
//!
//! 组件装配（加载/销毁）流程见同目录的 [`loader`]（以 `ComponentContainer` 的 impl 块提供），
//! 与容器查询分离：装配是启动期一次性过程，查询是运行期高频 API。
//! 容器级周期回调（全部就绪 / 销毁之前）见同目录的 [`lifecycle`]。
//!
//! 容器数据：
//! - `repository`：组件仓库（组件名 → 组件处理器）
//! - `trait_obj_cache`：trait object 缓存（(trait_type_id, 实例名) → 擦除对象 + vtable）
//! - `type_instance_names`：具体类型 → 实例名索引
//! - `trait_instance_names`：trait → 实现实例名索引
//! - `primary_by_type`：具体类型 → primary 实例名索引
//! - `order`：创建顺序队列（销毁时逆序使用）
//!
//! 全局配置不在容器中（由 `Application` 持有，经 `AppContext` 与 create 回调分发），
//! 配置查询函数见 `utils::core_util`。

mod loader;
pub(crate) mod lifecycle;

pub use lifecycle::{ComponentLifecycle, LifecycleRegistration};

use crate::model::condition::ComponentCondition;
use crate::utils::freeze_cell::FreezeCell;
use crate::utils::inner_util::get_short_type_name;
use crate::BoxFuture;
use async_trait::async_trait;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use toml::Value;

// =============================================================================
// 组件类型别名
// =============================================================================

/// trait object 访问器函数签名
///
/// 输入: `Arc<dyn Any + Send + Sync>`（组件实例的类型擦除形式）
/// 输出: `Option<TraitObjectEntry>`（coerce 后的 trait object + 还原用 vtable）
/// 由 `#[component]` on impl 块宏在编译期生成，已知具体类型和 trait，直接做类型转换。
pub type TraitObjAccessorFn =
    fn(Arc<dyn Any + Send + Sync>) -> Option<TraitObjectEntry>;

/// 组件唯一标识符：组件名（全局唯一，含跨具体类型）
///
/// 组件类型信息由 `ComponentProcessor::type_id()` 从实例派生，key 仅承载名字。
pub(crate) type ComponentKey = String;

/// 组件创建函数签名
///
/// 闭包接收组件容器（`Arc<ComponentContainer>`，create 阶段依赖解析用）
/// 与合并后的全局配置（`Arc<Value>`，配置组件反序列化用），
/// 返回一个 BoxFuture，解析为组件实例 T。
pub type CreateFn<T> = Box<
    dyn FnOnce(Arc<ComponentContainer>, Arc<Value>) -> BoxFuture<anyhow::Result<T>> + Send + Sync,
>;

/// 组件初始化函数签名
///
/// 接收组件的共享引用 `Arc<T>`（对应 `async fn init(&self)` 方法签名）：
/// init 阶段组件实例仍由组件仓库持有，初始化逻辑仅能读取/借用自身，不能消费。
pub type InitFn<T> = Box<dyn FnOnce(Arc<T>) -> BoxFuture<anyhow::Result<()>> + Send + Sync>;

/// 组件销毁函数签名
///
/// 接收「有所有权」的组件实例 `T`（对应 `async fn destroy(self)` 方法签名）：
/// 与 init 的共享引用不同，销毁阶段所有权已从仓库移出，
/// 方法内可消费字段、取出内部资源。要求调用前 Arc 引用计数为 1
/// （见 `ComponentWrapper::destroy` 的 `Arc::try_unwrap`）。
pub type DestroyFn<T> = Box<dyn FnOnce(T) -> BoxFuture<anyhow::Result<()>> + Send + Sync>;

// =============================================================================
// 组件系统错误
// =============================================================================

/// 组件系统相关的特定错误
#[derive(Debug, Error)]
pub enum ComponentError {
    #[error("Component not found for type: {type_name}, name: {name}")]
    NotFound { type_name: String, name: String },

    #[error("Failed to downcast component: {type_name}, name: {name}")]
    DowncastFailed { type_name: String, name: String },

    #[error("Component not initialized (inner is None) for type: {type_name}, name: {name}")]
    NotInitialized { type_name: String, name: String },

    #[error("Internal error: {message}")]
    InternalError { message: String },

    #[error("No implementation found for trait: {trait_name}")]
    TraitImplNotFound { trait_name: String },

    #[error("Ambiguous trait implementation for '{trait_name}': candidates = {candidates:?}")]
    AmbiguousTraitImpl {
        trait_name: String,
        candidates: Vec<String>,
    },

    #[error("Multiple component instances found for type '{type_name}': candidates = {candidates:?}")]
    AmbiguousComponent {
        type_name: String,
        candidates: Vec<String>,
    },
}

// =============================================================================
// 组件生命周期抽象
// =============================================================================

/// 组件处理器 Trait
///
/// 定义了组件生命周期的三个核心阶段：创建、初始化、销毁。
#[async_trait]
pub trait ComponentProcessor: Any + Send + Sync {
    /// 阶段一：创建实例
    ///
    /// 接收组件容器 `Arc<ComponentContainer>`（构造器注入在此阶段通过容器解析
    /// 依赖，拓扑排序保证依赖组件已创建并初始化完成）与合并后的全局配置
    /// `Arc<Value>`（配置组件反序列化用）。
    async fn create(
        &mut self,
        container: Arc<ComponentContainer>,
        config: Arc<Value>,
    ) -> anyhow::Result<()>;

    /// 阶段二：初始化（注入完成后立即执行，对应 Spring `@PostConstruct`）
    ///
    /// 每个组件在其 create 完成后立即 init，拓扑序保证依赖组件先完成
    /// create + init：init 内可安全访问全部依赖组件（实例存在、trait object
    /// 缓存已填充且已完成初始化）。
    /// 与 destroy 不同：init 以 `Arc<T>` 共享引用调用用户方法（`async fn init(&self)`），
    /// 组件实例仍由组件仓库持有，方法内仅能读取/借用自身，不能消费。
    async fn init(&mut self) -> anyhow::Result<()>;

    /// 阶段三：销毁（清理资源）
    ///
    /// 与 init 不同：destroy 以「有所有权」的实例 `T` 调用用户方法（`async fn destroy(self)`），
    /// 实例所有权已从仓库移出，方法内可消费字段、取出内部资源。
    async fn destroy(&mut self) -> anyhow::Result<()>;

    /// 用于类型转换
    fn as_any(&self) -> &dyn Any;

    /// 获取组件实例的类型擦除引用
    ///
    /// 仅在 `create` 完成后才返回 `Some`。
    /// 返回 `Arc<dyn Any + Send + Sync>`，可以 `downcast` 回具体类型。
    fn get_inner_arc_any(&self) -> Option<Arc<dyn Any + Send + Sync>>;

    /// 获取组件实例的具体类型
    ///
    /// 类型信息由实例本身派生（而非注册元数据），是组件类型的单一事实来源，
    /// 供 trait 依赖展开与按类型查询使用。
    fn type_id(&self) -> TypeId;
}

/// 所有可注入 trait 的 super_trait
///
/// 要求 trait object 必须满足 `Any + Send + Sync`，
/// 从而能够将 `Arc<dyn Trait>` 擦除为 `Arc<dyn Injectable>` 存入统一存储。
/// 所有具体的 `'static` 类型自动实现此 trait。
pub trait Injectable: Any + Send + Sync {}
impl<T: Any + Send + Sync> Injectable for T {}

/// trait object 缓存条目：类型擦除对象 + 还原用 vtable
///
/// trait 还原原理：Rust 的 trait object 是「数据指针 + vtable 指针」组成的 fat pointer，
/// vtable 布局属未规范化的实现细节，无法仅凭 `Arc<dyn Injectable>` 安全还原出
/// `Arc<dyn Trait>`。因此本条目在写入缓存时（accessor 内 upcasting coercion 的瞬间）
/// 记录编译器算出的 dyn Trait 真实 vtable 指针（'static 只读静态数据）；
/// 取用侧用「数据指针 + 记录的 vtable」拼回 fat pointer 即可安全还原，
/// 不依赖任何 vtable 布局假设。
pub struct TraitObjectEntry {
    /// 类型擦除后的组件 trait object（缓存中的持有者）
    pub obj: Arc<dyn Injectable>,
    /// coercion 生成的 dyn Trait 真实 vtable 指针（'static 只读静态数据）
    pub vtable: *const (),
}

impl Clone for TraitObjectEntry {
    fn clone(&self) -> Self {
        Self {
            obj: self.obj.clone(),
            vtable: self.vtable,
        }
    }
}

// SAFETY: `vtable` 指向编译器生成的 vtable 静态数据：只读、'static、无释放义务、
// 不携带所有权，仅作还原用的元数据指针；`obj` 本身 `Send + Sync`。
// 跨线程共享（存入容器冻结快照缓存）安全。
unsafe impl Send for TraitObjectEntry {}
unsafe impl Sync for TraitObjectEntry {}

// =============================================================================
// 编译期注册类型（宏生成 + inventory 收集）
// =============================================================================

/// 组件工厂结构体
///
/// 由 `#[component]` 等宏生成并经 `inventory` 收集，存储组件的元数据与构造逻辑。
/// 构造器返回 `Box<dyn ComponentProcessor>`（内部为 `ComponentWrapper<T>`）。
pub struct ComponentProcessorFactory {
    pub dependencies: &'static [&'static str],
    /// trait 依赖：直接存储 `TypeId::of::<dyn Trait>()`（const fn，static 初始化中直接求值）
    /// 用于拓扑排序中直接通过 TypeId 查找 trait 实现，无需字符串中转
    pub trait_dependencies: &'static [TypeId],
    /// 具体类型依赖：直接存储 `TypeId::of::<ConcreteType>()`（const fn 直接求值）
    /// 无名称注入 `Arc<T>` 时使用（组件可能自定义名称，短名不能作为依赖名）
    pub type_dependencies: &'static [TypeId],
    pub name: &'static str,
    /// 条件声明：None 表示无条件注册；Some 为惰性构造函数指针，
    /// 注册期调用一次求值，不满足则组件不注册（不参与后续创建）
    pub condition: Option<fn() -> ComponentCondition>,
    pub constructor: fn() -> Box<dyn ComponentProcessor>,
}

/// 编译期 trait 实现注册结构体（供 `inventory` 收集）
///
/// 由 `#[component]` 在 `impl Trait for Struct` 上生成，
/// 启动时被 `loader` 读入构建 trait 实现索引。
pub struct TraitImplRegistration {
    /// `TypeId::of::<dyn Trait>()`
    pub trait_type_id: TypeId,
    /// `TypeId::of::<ConcreteType>()`
    pub impl_type_id: TypeId,
    /// 类型转换函数：`Arc<ConcreteType> → Arc<dyn Injectable>`
    pub accessor: TraitObjAccessorFn,
}

/// 编译期 primary（首要）实例注册结构体（供 `inventory` 收集）
///
/// 由 `#[provider(primary)]` 生成，声明"该具体类型的首要实例"：
/// 当框架按类型获取组件时优先返回它。启动注册期被 `loader`
/// 读入构建 primary 索引，并校验名字对应的组件存在（其 provider 可能被
/// 条件过滤移除）、同类型 primary 唯一。
pub struct PrimaryRegistration {
    /// `TypeId::of::<ConcreteType>()`（const fn，static 初始化中直接求值）
    pub type_id: TypeId,
    /// primary 实例的组件名（与 `#[provider]` 的 name 参数同源，天然一致）
    pub name: &'static str,
}

// 自动收集所有标记了 ComponentProcessorFactory 的静态变量
inventory::collect!(ComponentProcessorFactory);

// 自动收集所有标记了 TraitImplRegistration 的静态变量
inventory::collect!(TraitImplRegistration);

// 自动收集所有标记了 PrimaryRegistration 的静态变量
inventory::collect!(PrimaryRegistration);

/// 组件包装器
///
/// 泛型 T 是具体的组件类型。该包装器管理用户提供的 create/init/destroy 闭包。
pub struct ComponentWrapper<T: Any + Send + Sync> {
    create_fn: Option<CreateFn<T>>,
    init_fn: Option<InitFn<T>>,
    destroy_fn: Option<DestroyFn<T>>,
    inner: Option<Arc<T>>, // 存储实际的组件实例
}

impl<T: Any + Send + Sync> ComponentWrapper<T> {
    pub fn new(
        create_fn: CreateFn<T>,
        init_fn: Option<InitFn<T>>,
        destroy_fn: Option<DestroyFn<T>>,
    ) -> Self {
        Self {
            create_fn: Some(create_fn),
            init_fn,
            destroy_fn,
            inner: None,
        }
    }
}

#[async_trait]
impl<T: Any + Send + Sync> ComponentProcessor for ComponentWrapper<T> {
    async fn create(
        &mut self,
        container: Arc<ComponentContainer>,
        config: Arc<Value>,
    ) -> anyhow::Result<()> {
        // 执行用户提供的创建函数，生成实例
        if let Some(create_fn) = self.create_fn.take() {
            let instance = create_fn(container, config).await?;
            // 将实例封装在 Arc 中，允许共享所有权
            self.inner = Some(Arc::new(instance));
        }
        Ok(())
    }

    async fn init(&mut self) -> anyhow::Result<()> {
        if let Some(init_fn) = self.init_fn.take() {
            // init 传入 Arc 克隆（共享引用，对应用户方法 &self 签名），
            // 实例所有权仍保留在仓库中；destroy 阶段才真正移交所有权
            if let Some(arc_t) = self.inner.as_ref() {
                init_fn(arc_t.clone()).await?;
            }
        }
        Ok(())
    }

    async fn destroy(&mut self) -> anyhow::Result<()> {
        // 销毁阶段所有权已从仓库移出：无论组件是否注册了销毁逻辑，
        // 都先尝试解包 Arc 校验引用计数为 1，及早暴露引用泄漏——
        // 未实现 destroy 的组件若被其他持有者引用（计数 > 1），实例
        // 同样无法释放，报错让开发者感知，而非静默泄漏
        if let Some(arc_t) = self.inner.take() {
            // 与 init 传入 Arc 共享引用不同：destroy 尝试解包 Arc 拿到
            // 「有所有权」的实例 T（对应用户方法 self 签名），供销毁逻辑
            // 消费字段、取出内部资源。只有当引用计数为 1 时（即没有
            // 其他地方持有该组件），才能成功解包并安全销毁
            match Arc::try_unwrap(arc_t) {
                Ok(t) => {
                    // 成功拿到 T 的所有权：若注册了销毁逻辑则执行
                    // （消费实例）；否则实例随 t 在此作用域结束自然释放
                    if let Some(destroy_fn) = self.destroy_fn.take() {
                        destroy_fn(t).await?;
                    }
                }
                Err(_arc_t) => {
                    // 失败：说明还有其他地方持有这个 Arc（可能是因为循环引用或逻辑泄露）
                    return Err(anyhow::anyhow!(
                        "Cannot destroy component: it is still in use by others (Arc strong_count > 1)"
                    ));
                }
            }
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn get_inner_arc_any(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        self.inner.as_ref().map(|arc| arc.clone() as Arc<dyn Any + Send + Sync>)
    }

    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
}

// =============================================================================
// 组件容器（定义 + 运行期查询 API；装配流程见 loader）
// =============================================================================

/// 组件容器：聚合全部组件数据并提供查询 API。
///
/// 存储采用 [`FreezeCell`]（一次性写入 + 一次性清空的不可变快照单元）：
/// - 装配期（load 流程）：BUILDING 态，单线程读写（装配器直接 get_mut）
/// - 运行期：load 末尾统一 `freeze` 为只读快照，查询零锁读
/// - 销毁期：shutdown 中 take（仓库/顺序）或 clear（缓存/索引），
///   发生在无并发读者的单线程时刻（FreezeCell 契约）
pub struct ComponentContainer {
    /// 组件仓库（组件名 → 组件处理器）
    pub(crate) repository: FreezeCell<HashMap<ComponentKey, Box<dyn ComponentProcessor>>>,
    /// trait object 缓存（(trait_type_id, 实例名) → 擦除对象 + 还原 vtable）
    pub(crate) trait_obj_cache: FreezeCell<HashMap<(TypeId, String), TraitObjectEntry>>,
    /// 具体类型 → 该类型所有已创建实例名
    pub(crate) type_instance_names: FreezeCell<HashMap<TypeId, Vec<String>>>,
    /// trait_type_id → 该 trait 所有已创建实现组件的实例名
    pub(crate) trait_instance_names: FreezeCell<HashMap<TypeId, Vec<String>>>,
    /// 具体类型 → 该类型的 primary（首要）实例名
    pub(crate) primary_by_type: FreezeCell<HashMap<TypeId, String>>,
    /// 组件创建顺序队列（销毁时逆序使用）
    pub(crate) order: FreezeCell<Vec<ComponentKey>>,
}

impl ComponentContainer {
    /// 创建空的组件容器（全部存储处于 BUILDING 态，供装配器构建）
    pub(crate) fn new() -> Self {
        Self {
            repository: FreezeCell::new(HashMap::new()),
            trait_obj_cache: FreezeCell::new(HashMap::new()),
            type_instance_names: FreezeCell::new(HashMap::new()),
            trait_instance_names: FreezeCell::new(HashMap::new()),
            primary_by_type: FreezeCell::new(HashMap::new()),
            order: FreezeCell::new(Vec::new()),
        }
    }

    // =========================================================================
    // 组件查询 API
    // =========================================================================

    /// 获取组件实例（按类型）
    ///
    /// 解析顺序：
    /// 1. primary 优先：该类型注册了首要实例（`#[provider(primary)]`）→ 直接返回
    /// 2. 默认名快速路径：组件名恰为类型短名 → 直接命中
    /// 3. 类型唯一性兜底：按具体类型收集全部实例名
    ///    - 恰好一个 → 返回该实例
    ///    - 多个 → `AmbiguousComponent`
    ///    - 零个 → `NotFound`
    pub fn get_component<T>(&self) -> Result<Arc<T>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();

        // 1. primary 优先：显式声明的首要实例高于命名约定
        if let Some(primary_name) = self
            .primary_by_type
            .get()
            .and_then(|m| m.get(&type_id))
            .cloned()
        {
            return self.get_component_by_name(primary_name);
        }

        // 2. 快速路径：默认命名（类型短名）直接命中
        let short_name = get_short_type_name::<T>();
        if self
            .repository
            .get()
            .is_some_and(|r| r.contains_key(&short_name))
        {
            return self.get_component_by_name(short_name);
        }

        // 3. 兜底：组件自定义了名称，按具体类型收集全部实例名
        let names = self
            .type_instance_names
            .get()
            .and_then(|m| m.get(&type_id))
            .cloned()
            .unwrap_or_default();
        match names.as_slice() {
            [] => Err(ComponentError::NotFound {
                type_name: std::any::type_name::<T>().to_string(),
                name: short_name,
            }),
            [name] => self.get_component_by_name(name.clone()),
            _ => Err(ComponentError::AmbiguousComponent {
                type_name: std::any::type_name::<T>().to_string(),
                candidates: names,
            }),
        }
    }

    /// 获取组件实例（按名称）
    pub fn get_component_by_name<T, S>(&self, name: S) -> Result<Arc<T>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
        S: Into<String>,
    {
        let name_str = name.into();

        if let Some(processor) = self.repository.get().and_then(|r| r.get(&name_str)) {
            let as_any = processor.as_any();

            if let Some(wrapper) = as_any.downcast_ref::<ComponentWrapper<T>>() {
                if let Some(ref inner) = wrapper.inner {
                    Ok(inner.clone())
                } else {
                    Err(ComponentError::NotInitialized {
                        type_name: std::any::type_name::<T>().to_string(),
                        name: name_str,
                    })
                }
            } else {
                Err(ComponentError::DowncastFailed {
                    type_name: std::any::type_name::<T>().to_string(),
                    name: name_str,
                })
            }
        } else {
            Err(ComponentError::NotFound {
                type_name: std::any::type_name::<T>().to_string(),
                name: name_str,
            })
        }
    }

    /// 获取某类型的所有组件
    pub fn get_component_by_type<T>(&self) -> Result<Vec<Arc<T>>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
    {
        let target_type_id = TypeId::of::<T>();
        let mut results = Vec::new();

        let Some(repo) = self.repository.get() else {
            return Ok(results);
        };
        for (name, processor) in repo.iter() {

            // 类型信息取自处理器本身（`ComponentProcessor::type_id`）；完全限定语法消除 Any::type_id 歧义
            if ComponentProcessor::type_id(&**processor) == target_type_id {
                if let Some(wrapper) = processor.as_any().downcast_ref::<ComponentWrapper<T>>() {
                    if let Some(ref inner) = wrapper.inner {
                        results.push(inner.clone());
                    } else {
                        return Err(ComponentError::NotInitialized {
                            type_name: std::any::type_name::<T>().to_string(),
                            name: name.clone(),
                        });
                    }
                } else {
                    return Err(ComponentError::DowncastFailed {
                        type_name: std::any::type_name::<T>().to_string(),
                        name: name.clone(),
                    });
                }
            }
        }

        Ok(results)
    }

    // =========================================================================
    // Trait object 注入 API
    // =========================================================================

    /// 按 trait + name 获取组件 → `Arc<dyn Trait>`
    ///
    /// 从 `trait_obj_cache` 取出 `TraitObjectEntry`，用注册时记录的 dyn Trait
    /// 真实 vtable 与实例数据指针拼回 fat pointer，重建 `Arc<dyn Trait>`。
    /// vtable 是 accessor 内 coercion 时编译器算出的真实值，与缓存条目同源
    /// （同一注册条目的 trait_type_id + accessor）。
    ///
    /// # Safety
    ///
    /// 内部使用 unsafe 将拆解后的 fat pointer 位重新拼回 `Arc<dyn Trait>`。
    /// 安全性由以下不变量保证：
    /// - data 指针由 `Arc::into_raw` 在取用侧创建（指向 ArcInner）
    /// - vtable 是注册时 coercion 生成的 dyn Trait 真实 vtable（'static 只读静态数据），
    ///   其 [drop, size, align] 头三槽与该具体类型一致，`Arc::from_raw` 的 drop 行为正确
    pub fn get_component_by_trait_and_name<Trait: Injectable + ?Sized>(
        &self,
        name: &str,
    ) -> Result<Arc<Trait>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let cache_key = (trait_type_id, name.to_string());

        let entry = self
            .trait_obj_cache
            .get()
            .and_then(|m| m.get(&cache_key))
            .ok_or_else(|| ComponentError::TraitImplNotFound {
                trait_name: std::any::type_name::<Trait>().to_string(),
            })?
            .clone();

        // 拆出 data 指针（ArcInner），与注册时记录的 dyn Trait 真实 vtable 拼回 fat pointer
        let ptr_injectable: *const dyn Injectable = Arc::into_raw(entry.obj);
        // SAFETY: fat pointer 位拆解（data + vtable 两段 usize），仅观察用途
        let bits: [usize; 2] = unsafe { std::mem::transmute_copy(&ptr_injectable) };
        // SAFETY: 拼出的 fat pointer 与编译器 upcast 产物位级相同——data 来自
        // `Arc::into_raw`，vtable 是 coercion 生成的真实 dyn Trait vtable，满足
        // `Arc::from_raw` 契约（head 三槽 drop/size/align 与该具体类型一致）。
        let ptr_trait: *const Trait =
            unsafe { std::mem::transmute_copy(&[bits[0], entry.vtable as usize]) };
        Ok(unsafe { Arc::from_raw(ptr_trait) })
    }

    /// 按 trait 获取唯一实现 → `Arc<dyn Trait>`
    ///
    /// 通过 trait 实现索引获取该 trait 的所有实例名，要求恰好一个。
    ///
    /// 错误：
    /// - 0 个实现类型 → `TraitImplNotFound`
    /// - 多个实现类型或实例 → `AmbiguousTraitImpl`
    pub fn get_component_by_trait<Trait: Injectable + ?Sized>(
        &self,
    ) -> Result<Arc<Trait>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let trait_name = std::any::type_name::<Trait>().to_string();

        let names = self
            .trait_instance_names
            .get()
            .and_then(|m| m.get(&trait_type_id))
            .cloned()
            .unwrap_or_default();
        if names.is_empty() {
            return Err(ComponentError::TraitImplNotFound { trait_name });
        }
        if names.len() > 1 {
            return Err(ComponentError::AmbiguousTraitImpl {
                trait_name,
                candidates: names.clone(),
            });
        }

        self.get_component_by_trait_and_name::<Trait>(&names[0])
    }

    /// 按 trait 获取所有实现 → `Vec<Arc<dyn Trait>>`
    ///
    /// 通过 trait 实现索引获取该 trait 的所有实例名，逐个从 `trait_obj_cache` 取出。
    ///
    /// 如果 trait 没有实现类型或所有实例都未被缓存，返回空 Vec。
    pub fn get_components_by_trait<Trait: Injectable + ?Sized>(
        &self,
    ) -> Result<Vec<Arc<Trait>>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let trait_name = std::any::type_name::<Trait>().to_string();

        let names = self
            .trait_instance_names
            .get()
            .and_then(|m| m.get(&trait_type_id))
            .cloned()
            .unwrap_or_default();

        let mut results = Vec::new();
        for name in names.iter() {
            match self.get_component_by_trait_and_name::<Trait>(name) {
                Ok(entry) => results.push(entry),
                Err(_) => {
                    tracing::debug!(
                        "Trait '{}' instance '{}' not found in cache, skipped",
                        trait_name,
                        name
                    );
                }
            }
        }

        Ok(results)
    }

    // =========================================================================
    // 生命周期辅助
    // =========================================================================

    /// 组件仓库是否为空（供 `Application::shutdown` 判断是否需要销毁组件）
    pub(crate) fn is_empty(&self) -> bool {
        self.repository.get().map(|r| r.is_empty()).unwrap_or(true)
    }
}
