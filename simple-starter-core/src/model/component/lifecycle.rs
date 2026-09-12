//! # 组件容器级生命周期回调
//!
//! 与 [`ComponentProcessor`](super::ComponentProcessor) 的 bean 级三阶段
//! （create/init/destroy）不同，本模块定义**容器级**批次周期（对标 Spring
//! `SmartInitializingSingleton` 与 `SmartLifecycle`）：组件按需实现
//! [`ComponentLifecycle`]，由装配流程（见 [`loader`](super::loader)）在
//! 对应时机批次执行。

use super::{ComponentContainer, Injectable};
use async_trait::async_trait;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// 组件容器级生命周期回调（可选实现，对应 Spring 容器级周期）
///
/// 与 bean 级三阶段（create/init/destroy，见 [`ComponentProcessor`](super::ComponentProcessor)）
/// 不同，本 trait 定义**容器级**批次回调：
///
/// - [`after_all_ready`](Self::after_all_ready)：全部 bean 完成注入与初始化后，
///   按创建顺序正序批次执行（对应 Spring `SmartInitializingSingleton`）。
/// - [`before_destroy`](Self::before_destroy)：全部 bean 准备销毁之前，按创建
///   顺序逆序批次执行（对应 Spring `SmartLifecycle.stop()`）；此时全局缓存
///   尚未清空、全部 bean 存活，可互相解析。
///
/// 两个方法均有默认空实现，组件按需覆写。签名带容器 `Arc` 引用（与
/// `AppContext::container()` 返回形态一致）：回调内 deref 直接查询任意
/// 组件（动态查询场景），声明式依赖仍走字段注入；需要传播所有权时
/// 显式 `.clone()`（clone 点即所有权决策点，外传组件 Arc 需自行负责其
/// 生命周期约束，避免破坏销毁阶段的引用计数检查）。
/// 实现本 trait 的 impl 块须标注 `#[lifecycle]` 属性宏以生成 inventory 注册。
#[async_trait]
pub trait ComponentLifecycle: Injectable {
    /// 全部 bean 完成注入与初始化后执行（按创建顺序正序批次）
    async fn after_all_ready(&self, _container: &Arc<ComponentContainer>) -> anyhow::Result<()> {
        Ok(())
    }

    /// 全部 bean 准备销毁之前执行（按创建顺序逆序批次，缓存清空前）
    async fn before_destroy(&self, _container: &Arc<ComponentContainer>) -> anyhow::Result<()> {
        Ok(())
    }
}

/// 容器级生命周期实现注册（由 `#[lifecycle]` 宏生成，供 `inventory` 收集）
///
/// `accessor` 将类型擦除的组件实例还原为 `Arc<dyn ComponentLifecycle>`：
/// safe downcast 到具体类型后正向 coercion，与
/// [`TraitImplRegistration`](super::TraitImplRegistration) 的 accessor 同构，
/// 但直接返回 trait object，无需 vtable 拆解。
pub struct LifecycleRegistration {
    /// `TypeId::of::<ConcreteType>()`
    pub impl_type_id: TypeId,
    /// 类型转换函数：`Arc<dyn Any + Send + Sync>` → `Arc<dyn ComponentLifecycle>`
    pub accessor: fn(Arc<dyn Any + Send + Sync>) -> Option<Arc<dyn ComponentLifecycle>>,
}

// 自动收集所有标记了 LifecycleRegistration 的静态变量
inventory::collect!(LifecycleRegistration);

/// 构建生命周期实现索引：实现类型 → 匹配的注册列表（启动期局部使用）
///
/// 供装配流程按组件具体类型直接查询，避免对全量注册的线性扫描。
/// 一个具体类型至多一个 `ComponentLifecycle` 实现（coherence 保证），
/// 索引值用列表以与 trait 实现索引结构对齐。
pub(crate) fn build_lifecycle_index() -> HashMap<TypeId, Vec<&'static LifecycleRegistration>> {
    let mut index: HashMap<TypeId, Vec<&'static LifecycleRegistration>> = HashMap::new();
    for reg in inventory::iter::<LifecycleRegistration> {
        index.entry(reg.impl_type_id).or_default().push(reg);
    }
    index
}
