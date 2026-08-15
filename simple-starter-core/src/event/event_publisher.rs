//! 事件发布器：trait + 默认实现（框架内置，条件注册）。

use crate::core::app_component::Injectable;
use crate::event::app_event::{AppEvent, EventListenerRegistration};
use crate::event::event_listener::AnyEventListener;
use crate::global_state::{TRAIT_OBJ_CACHE, TYPE_INSTANCE_NAMES};
use crate::{component, injectable};
use async_trait::async_trait;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, Weak};

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

/// 监听器条目。
///
/// 弱引用断环设计：监听器组件常通过 `#[inject] Arc<dyn EventPublisher>` 注入
/// 发布器（对发布器持强引用）。若发布器再以强引用持有监听器，则两者互持
/// 形成引用环：组件永远无法释放，且销毁阶段 `Arc::try_unwrap`（要求计数为 1）
/// 必然失败。因此发布器对监听器仅存 Weak 断开此环——监听器组件由组件仓库
/// 强持有至应用结束，Weak 随时可升级；销毁阶段仓库先释放监听器，
/// 分派时 upgrade 失败即自动跳过。
#[derive(Clone)]
struct ListenerEntry {
    /// 弱引用仓库中的组件实例，分派时经 adapter 还原为 `dyn AnyEventListener`。
    /// 持弱引用而非强引用是为断开上述引用环（环的组成见结构体文档）。
    weak: Weak<dyn Injectable>,
    /// 类型桥接：`Arc<dyn Injectable>` → `Arc<dyn AnyEventListener>`（downcast 还原链路）。
    adapter: fn(Arc<dyn Injectable>) -> Option<Arc<dyn AnyEventListener>>,
}

impl ListenerEntry {
    /// 升级为可调用的监听器（升级失败或类型不匹配返回 `None`）。
    fn upgrade_listener(&self) -> Option<Arc<dyn AnyEventListener>> {
        (self.adapter)(self.weak.upgrade()?)
    }
}

/// 默认事件发布器。
///
/// 条件注册：仅当用户未提供任何 [`EventPublisher`] 实现时注册本默认实现，
/// 否则自动退位让位给用户实现（`on_missing_trait` 语义）。
///
/// 组件名显式指定为 `defaultEventPublisher`（而非结构体默认名），
/// 遵循插件默认实现命名约束，避免与用户同名结构体在注册期撞名。
#[component(
    name = "defaultEventPublisher",
    condition = crate::ComponentCondition::on_missing_trait::<dyn EventPublisher>(),
    init_method = "collect_listeners"
)]
pub struct DefaultEventPublisher {
    /// 事件类型 → 监听器列表（保持收集顺序），init 阶段写入一次，之后只读。
    listeners: OnceLock<HashMap<TypeId, Vec<ListenerEntry>>>,
}

impl DefaultEventPublisher {
    /// 收集所有 `#[event_listener]` 注册的监听器，构建事件类型索引。
    ///
    /// 在组件 init 阶段调用：全量组件 create 完成后 `TRAIT_OBJ_CACHE`
    /// 与 `TYPE_INSTANCE_NAMES` 已填充，收集必然命中。
    pub async fn collect_listeners(&self) -> anyhow::Result<()> {
        let mut map: HashMap<TypeId, Vec<ListenerEntry>> = HashMap::new();

        for reg in inventory::iter::<EventListenerRegistration> {
            // 实现组件的全部已创建实例
            let Some(names) = TYPE_INSTANCE_NAMES.get(&reg.impl_type_id) else {
                continue;
            };
            for name in names.iter() {
                let cache_key = (reg.listener_trait_type_id, name.clone());
                let Some(arc) = TRAIT_OBJ_CACHE.get(&cache_key) else {
                    continue;
                };
                // 验证 adapter 还原链路可通（失败时跳过），Weak 指向仓库持有的
                // 组件实例（应用生命周期内有效），分派时再 upgrade + adapter 还原
                let Some(listener) = (reg.adapter)(arc.value().obj.clone()) else {
                    continue;
                };
                // 登记日志：发布器收集到的监听器实例名与监听事件类型
                tracing::debug!(
                    "Collected event listener '{}' for {:?}",
                    name,
                    listener.event_type_id()
                );
                map.entry(reg.event_type_id).or_default().push(ListenerEntry {
                    weak: Arc::downgrade(&arc.value().obj),
                    adapter: reg.adapter,
                });
            }
        }

        self.listeners
            .set(map)
            .map_err(|_| anyhow::anyhow!("Event listener index already initialized"))?;
        Ok(())
    }
}

#[injectable]
#[async_trait]
impl EventPublisher for DefaultEventPublisher {
    async fn publish(&self, event: Arc<dyn AppEvent>) -> anyhow::Result<()> {
        // 完全限定语法取分桶键（AppEvent 内嵌 Any 槽位）
        let event_type_id = Any::type_id(&*event);

        // init 阶段已收集；未初始化时按无监听器处理
        let listeners = self
            .listeners
            .get()
            .and_then(|map| map.get(&event_type_id))
            .cloned()
            .unwrap_or_default();

        for entry in listeners {
            // 升级为临时强引用，保证调用期间存活；组件已销毁（Weak 失效）则跳过。
            // 这是 Weak 断环设计允许的行为：监听器生命周期由组件仓库独立管理，
            // 发布器不参与监听器的存亡
            let Some(listener) = entry.upgrade_listener() else {
                continue;
            };
            if let Err(e) = listener.on_event_any(event.clone()).await {
                tracing::error!(
                    "Event listener failed for '{}': {:#}",
                    std::any::type_name_of_val(event.as_ref()),
                    e
                );
            }
        }
        Ok(())
    }
}
