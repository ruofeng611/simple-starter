use crate::core::app_types::{CreateFn, DestroyFn, InitFn};
use async_trait::async_trait;
use std::any::{Any, TypeId};
use std::sync::Arc;
use crate::TraitObjAccessorFn;
use crate::loaders::component_condition::ComponentCondition;

/// 组件处理器 Trait
///
/// 定义了组件生命周期的三个核心阶段：创建、初始化、销毁。
#[async_trait]
pub trait ComponentProcessor: Any + Send + Sync {
    /// 阶段一：创建实例（此时不应访问其他依赖组件）
    async fn create(&mut self) -> anyhow::Result<()>;

    /// 阶段二：初始化（可以安全地获取并使用其他依赖组件）
    ///
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
// 跨线程共享（存入 `DashMap` 缓存）安全。
unsafe impl Send for TraitObjectEntry {}
unsafe impl Sync for TraitObjectEntry {}

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
    /// primary 依赖：直接存储 `TypeId::of::<ConcreteType>()`，由 `#[inject_primary]` 生成
    /// DFS 展开时仅建边到该类型的 primary 实例，不强制创建同类型其他实例
    pub primary_dependencies: &'static [TypeId],
    pub name: &'static str,
    /// 条件声明：None 表示无条件注册；Some 为惰性构造函数指针，
    /// 注册期调用一次求值，不满足则组件不注册（不参与 DFS 创建）
    pub condition: Option<fn() -> ComponentCondition>,
    pub constructor: fn() -> Box<dyn ComponentProcessor>,
}

/// 编译期 trait 实现注册结构体（供 `inventory` 收集）
///
/// 由 `#[component]` 在 `impl Trait for Struct` 上生成，
/// 启动时被 `component_loader` 读入构建 trait 实现索引。
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
/// 由 `#[primary]` 在 provider 函数上生成，声明"该具体类型的首要实例"：
/// 当框架按类型获取组件时优先返回它。启动注册期被 `component_loader`
/// 读入构建 primary 索引，并校验名字对应的组件存在、同类型 primary 唯一。
pub struct PrimaryRegistration {
    /// `TypeId::of::<ConcreteType>()`（const fn，static 初始化中直接求值）
    pub type_id: TypeId,
    /// primary 实例的组件名（必须与 `#[provider]` 注册的组件名一致）
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
    pub create_fn: Option<CreateFn<T>>,
    pub init_fn: Option<InitFn<T>>,
    pub destroy_fn: Option<DestroyFn<T>>,
    pub inner: Option<Arc<T>>, // 存储实际的组件实例
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
    async fn create(&mut self) -> anyhow::Result<()> {
        // 执行用户提供的创建函数，生成实例
        if let Some(create_fn) = self.create_fn.take() {
            let instance = create_fn().await?;
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
        if let Some(destroy_fn) = self.destroy_fn.take() {
            if let Some(arc_t) = self.inner.take() {
                // 与 init 传入 Arc 共享引用不同：destroy 尝试解包 Arc 拿到
                // 「有所有权」的实例 T（对应用户方法 self 签名），供销毁逻辑
                // 消费字段、取出内部资源。只有当引用计数为 1 时（即没有
                // 其他地方持有该组件），才能成功解包并安全销毁
                match Arc::try_unwrap(arc_t) {
                    Ok(t) => {
                        // 成功拿到 T 的所有权，执行销毁逻辑
                        destroy_fn(t).await?;
                    }
                    Err(_arc_t) => {
                        // 失败：说明还有其他地方持有这个 Arc（可能是因为循环引用或逻辑泄露）
                        return Err(anyhow::anyhow!(
                            "Cannot destroy component: it is still in use by others (Arc strong_count > 1)"
                        ));
                    }
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
