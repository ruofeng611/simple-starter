//! # 组件工厂描述结构
//!
//! 用于自动注册组件：每个 `#[auto_component]` 函数会生成一个 `ComponentFactory`，
//! 在启动时被 `component_loader` 调用以创建组件实例。

use std::any::{Any, TypeId};
use std::sync::Arc;

/// 组件工厂描述
#[derive(Clone, Copy)]
pub struct ComponentFactory {
    /// 组件注册名称
    pub name: &'static str,
    /// 组件类型 ID
    pub type_id: TypeId,
    /// 构造函数：返回 `Arc<dyn Any + Send + Sync>`
    pub constructor: fn() -> Arc<dyn Any + Send + Sync>,
}

// 通过 inventory 自动收集所有 ComponentFactory 实例
inventory::collect!(ComponentFactory);
