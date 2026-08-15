//! 应用事件：事件标记 trait 与监听器注册结构。

use crate::core::app_component::Injectable;
use crate::event::event_listener::AnyEventListener;
use std::any::{Any, TypeId};
use std::sync::Arc;

/// 所有事件类型的标记 trait。
///
/// 事件发布时被擦除为 `Arc<dyn Any + Send + Sync>` 统一分发，
/// 因此事件类型需满足 `Any + Send + Sync`。
/// 所有 `'static + Send + Sync` 具体类型自动实现此 trait，无需手动声明。
pub trait AppEvent: Any + Send + Sync {}

impl<T: Any + Send + Sync> AppEvent for T {}

/// 编译期事件监听器注册，由 `#[event_listener]` 宏在 impl 块上生成。
///
/// 发布器 init 阶段遍历收集，按事件类型 `event_type_id` 构建监听器索引。
pub struct EventListenerRegistration {
    /// `TypeId::of::<E>()`：监听的事件类型
    pub event_type_id: TypeId,
    /// `TypeId::of::<dyn EventListener<E>>()`：监听器 trait 的 TypeId（查 `TRAIT_OBJ_CACHE` 用）
    pub listener_trait_type_id: TypeId,
    /// `TypeId::of::<ImplType>()`：实现组件具体类型（查实例名索引用）
    pub impl_type_id: TypeId,
    /// 桥接适配器构造：`Arc<dyn Injectable> → Option<Arc<dyn AnyEventListener>>`
    ///
    /// 实现为 downcast 还原链路（全程 safe）：
    /// 正向 upcast 到 `Any` → `downcast::<ImplType>()` → 正向 coercion 到
    /// `dyn EventListener<E>`，不依赖任何 vtable 布局假设；
    /// downcast 失败（类型不匹配）返回 `None`，由收集方跳过。
    pub adapter: fn(Arc<dyn Injectable>) -> Option<Arc<dyn AnyEventListener>>,
}

// 自动收集所有标记了 EventListenerRegistration 的静态变量
inventory::collect!(EventListenerRegistration);
