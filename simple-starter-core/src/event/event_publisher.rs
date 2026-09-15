//! 事件发布器：trait + 默认实现（框架内置，条件注册）。

use crate::model::component::Injectable;
use crate::model::component::ComponentContainer;
use crate::model::component::ComponentLifecycle;
use crate::event::app_event::{AppEvent, EventListenerRegistration};
use crate::event::event_listener::AnyEventListener;
use crate::utils::freeze_cell::FreezeCell;
use crate::{component, injectable, lifecycle};
use async_trait::async_trait;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

/// 事件发布器 trait。
///
/// 组件通过 `#[inject] Arc<dyn EventPublisher>` 注入使用。
/// 必须显式继承 `Injectable`（框架约束：可注入 trait 的 super_trait）。
///
/// 核心方法保持 dyn 兼容（无泛型参数），以便生成 `dyn EventPublisher` 的 vtable；
/// 类型化便捷方法见 [`EventPublisherExt`]。
#[async_trait]
pub trait EventPublisher: Injectable {
    /// 发布事件：同步广播给该事件类型的所有监听器。
    ///
    /// 接收类型擦除后的 `Arc<dyn AppEvent>`，按具体事件类型的 `type_id`
    /// 分桶分派。单个监听器失败仅记录日志，不中断广播。
    async fn publish(&self, event: Arc<dyn AppEvent>) -> anyhow::Result<()>;
}

/// [`EventPublisher`] 的类型化便捷扩展。
///
/// 泛型方法不能定义在 `EventPublisher` 上：会破坏 dyn 兼容性，
/// 无法生成 `dyn EventPublisher` 的 vtable。因此以扩展 trait 提供，
/// blanket impl 对所有实现者生效。
#[async_trait]
pub trait EventPublisherExt: EventPublisher {
    /// 发布具体事件 `E`（自动擦除为 `Arc<dyn AppEvent>` 分派）。
    async fn publish_event<E: AppEvent>(&self, event: E) -> anyhow::Result<()> {
        self.publish(Arc::new(event)).await
    }
}

impl<T: EventPublisher + ?Sized> EventPublisherExt for T {}

/// 默认事件发布器。
///
/// 条件注册：仅当用户未提供任何 [`EventPublisher`] 实现时注册本默认实现，
/// 否则自动退位让位给用户实现（`on_missing_trait` 语义）。
///
/// 组件名显式指定为 `defaultEventPublisher`（而非结构体默认名），
/// 遵循插件默认实现命名约束，避免与用户同名结构体在注册期撞名。
///
/// 监听器索引为强引用快照（见 [`DefaultEventPublisher::listeners`] 字段
/// 文档）：`after_all_ready` 批次收集并冻结，`before_destroy` 批次清空。
#[component(
    name = "defaultEventPublisher",
    condition = crate::ComponentCondition::on_missing_trait::<dyn EventPublisher>()
)]
pub(crate) struct DefaultEventPublisher {
    /// 事件类型 → 监听器列表（保持收集顺序）。
    ///
    /// 生命周期：`after_all_ready` 批次收集一次（`get_mut` 填充）→ 冻结
    /// （运行期分派零锁读）→ `before_destroy` 批次清空一次（释放全部
    /// 监听器强引用，断开“监听器 ↔ 发布器”引用环）。
    /// 列表以 `Arc<Vec<_>>` 承载：`publish` 克隆外层 Arc（单次计数、零堆
    /// 分配）后跨 await 分派——分派期间数据由 Arc 计数保活，借用不跨
    /// await，销毁期清空与新读的并发窗口仅存在于 get 内部瞬间。
    listeners: FreezeCell<HashMap<TypeId, Arc<Vec<Arc<dyn AnyEventListener>>>>>,
}

impl DefaultEventPublisher {
    /// 收集所有 `#[event_listener]` 注册的监听器，构建事件类型索引。
    ///
    /// 由本组件 `after_all_ready` 批次调用（容器全部组件 create 完成后的
    /// 冻结视图查询）：`trait_obj_cache` 与 `type_instance_names` 已填充，
    /// 收集必然命中。
    fn collect_listeners(&self, container: &ComponentContainer) -> anyhow::Result<()> {
        let Some(map) = self.listeners.get_mut() else {
            return Ok(()); // 索引已冻结（重复调用防御）
        };

        for reg in inventory::iter::<EventListenerRegistration> {
            // 实现组件的全部已创建实例
            let Some(names) = container
                .type_instance_names
                .get()
                .and_then(|m| m.get(&reg.impl_type_id))
            else {
                continue;
            };
            for name in names.iter() {
                let cache_key = (reg.listener_trait_type_id, name.clone());
                let Some(arc) = container
                    .trait_obj_cache
                    .get()
                    .and_then(|m| m.get(&cache_key))
                else {
                    continue;
                };
                // adapter 还原出 `Arc<dyn AnyEventListener>`，其内部即组件实例的
                // 强引用：索引直接持有，分派免 upgrade；销毁前由 before_destroy
                // 清空断环
                let Some(listener) = (reg.adapter)(arc.obj.clone()) else {
                    continue;
                };
                // 登记日志：发布器收集到的监听器实例名与监听事件类型
                tracing::debug!(
                    "Collected event listener '{}' for {:?}",
                    name,
                    listener.event_type_id()
                );
                Arc::make_mut(map.entry(reg.event_type_id).or_default()).push(listener);
            }
        }
        Ok(())
    }
}

/// 容器级生命周期：监听器索引的收集与清空时机
///
/// - [`ComponentLifecycle::after_all_ready`]：全部组件就绪后收集监听器并
///   冻结索引（此后运行期分派零锁读）
/// - [`ComponentLifecycle::before_destroy`]：销毁前清空监听器强引用，
///   断开"监听器 ↔ 发布器"引用环（组件销毁时 Arc 计数归 1）
#[lifecycle]
#[async_trait]
impl ComponentLifecycle for DefaultEventPublisher {
    async fn after_all_ready(&self, container: &Arc<ComponentContainer>) -> anyhow::Result<()> {
        self.collect_listeners(container)?;
        // 收集完成：冻结索引为不可变快照，此后运行期分派零锁读
        self.listeners.freeze();
        Ok(())
    }

    async fn before_destroy(&self, _container: &Arc<ComponentContainer>) -> anyhow::Result<()> {
        // 清空监听器强引用：断开"监听器 ↔ 发布器"引用环（销毁循环前批次执行，
        // 此时全部组件存活、无并发读者）
        self.listeners.clear();
        Ok(())
    }
}

#[injectable]
#[async_trait]
impl EventPublisher for DefaultEventPublisher {
    async fn publish(&self, event: Arc<dyn AppEvent>) -> anyhow::Result<()> {
        // 完全限定语法取分桶键（AppEvent 内嵌 Any 槽位）
        let event_type_id = Any::type_id(&*event);

        // 收集阶段已写入（after_all_ready 批次）；未收集时按无监听器处理。
        // 克隆外层 Arc（单次计数、零堆分配）后跨 await 分派：分派期间
        // 数据由 Arc 计数保活，借用不跨 await
        if let Some(listeners) = self
            .listeners
            .get()
            .and_then(|map| map.get(&event_type_id))
            .cloned()
        {
            for listener in listeners.iter() {
                // 强引用监听器：before_destroy 清空前始终存活，直接分派
                if let Err(e) = listener.on_event_any(event.clone()).await {
                    tracing::error!(
                        "Event listener failed for '{}': {:#}",
                        std::any::type_name_of_val(event.as_ref()),
                        e
                    );
                }
            }
        }
        Ok(())
    }
}
