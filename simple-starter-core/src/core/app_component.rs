use crate::core::app_types::{CreateFn, DestroyFn, InitFn};
use async_trait::async_trait;
use std::any::{Any, TypeId};
use std::sync::Arc;

/// 组件处理器 Trait
///
/// 定义了组件生命周期的三个核心阶段：创建、初始化、销毁。
#[async_trait]
pub trait ComponentProcessor: Any + Send + Sync {
    /// 阶段一：创建实例（此时不应访问其他依赖组件）
    async fn create(&mut self) -> anyhow::Result<()>;

    /// 阶段二：初始化（可以安全地获取并使用其他依赖组件）
    async fn init(&mut self) -> anyhow::Result<()>;

    /// 阶段三：销毁（清理资源）
    async fn destroy(&mut self) -> anyhow::Result<()>;

    /// 用于类型转换
    fn as_any(&self) -> &dyn Any;
}

/// 组件工厂结构体
///
/// 用于 `inventory` 收集，存储组件的元数据和构造逻辑。
pub struct ComponentProcessorFactory {
    pub dependencies: &'static [&'static str],
    pub name: &'static str,
    pub type_id: TypeId,
    pub constructor: fn() -> Box<dyn ComponentProcessor>,
}

// 自动收集所有标记了 ComponentProcessorFactory 的静态变量
inventory::collect!(ComponentProcessorFactory);

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
            // 将 Arc 克隆一份传给初始化函数
            if let Some(arc_t) = self.inner.as_ref() {
                init_fn(arc_t.clone()).await?;
            }
        }
        Ok(())
    }

    async fn destroy(&mut self) -> anyhow::Result<()> {
        if let Some(destroy_fn) = self.destroy_fn.take() {
            if let Some(arc_t) = self.inner.take() {
                // 尝试解包 Arc 获取唯一所有权
                // 只有当引用计数为 1 时（即没有其他地方持有该组件），才能成功解包并安全销毁
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
}
