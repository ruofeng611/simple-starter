//! 事件监听器：泛型监听 trait + 内部统一分派 trait + 类型桥接适配器。

use crate::core::app_component::Injectable;
use crate::event::app_event::AppEvent;
use crate::BoxFuture;
use async_trait::async_trait;
use std::any::{Any, TypeId};
use std::sync::Arc;

/// 用户实现的类型化监听器 trait。
///
/// 实现此 trait 的组件配合 `#[event_listener]` 宏注册后，
/// 发布器在 init 阶段自动收集（无需手动注册）。
/// 必须显式继承 `Injectable`（框架约束：可注入 trait 的 super_trait）。
#[async_trait]
pub trait EventListener<E: AppEvent>: Injectable {
    /// 处理事件。
    async fn on_event(&self, event: &E) -> anyhow::Result<()>;
}

/// 内部统一分派 trait（非泛型，可擦除）。
///
/// 发布器以 `Arc<dyn AnyEventListener>` 统一存储全部监听器，
/// 按事件类型 `TypeId` 分桶后在分派时还原为具体事件。
#[async_trait]
pub trait AnyEventListener: Injectable {
    /// 返回所监听事件的具体类型（发布器分桶索引键）。
    fn event_type_id(&self) -> TypeId;

    /// 以类型擦除形式处理事件。
    fn on_event_any(&self, event: Arc<dyn Any + Send + Sync>) -> BoxFuture<anyhow::Result<()>>;
}

/// 泛型监听器到统一分派接口的桥接适配器。
///
/// 由 `#[event_listener]` 宏生成的 adapter 构造，将
/// `Arc<dyn EventListener<E>>` 包装为 `Arc<dyn AnyEventListener>`。
pub struct TypedListenerAdapter<E: AppEvent> {
    pub inner: Arc<dyn EventListener<E>>,
}

#[async_trait]
impl<E: AppEvent> AnyEventListener for TypedListenerAdapter<E> {
    fn event_type_id(&self) -> TypeId {
        TypeId::of::<E>()
    }

    fn on_event_any(&self, event: Arc<dyn Any + Send + Sync>) -> BoxFuture<anyhow::Result<()>> {
        // 先 clone inner 再 move 进异步块：BoxFuture 的 trait object 生命周期默认为
        // 'static，async 块不能借用 &self
        let inner = self.inner.clone();
        Box::pin(async move {
            let typed = event.downcast::<E>().map_err(|_| {
                anyhow::anyhow!(
                    "Event type mismatch: expected '{}'",
                    std::any::type_name::<E>()
                )
            })?;
            inner.on_event(&typed).await
        })
    }
}
