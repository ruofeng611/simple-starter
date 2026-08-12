use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{Layer, Registry};
use crate::core::app_component::Injectable;

/// trait object 访问器函数签名
///
/// 输入: `Arc<dyn Any + Send + Sync>`（组件实例的类型擦除形式）
/// 输出: `Option<Arc<dyn Injectable>>`（通过 upcasting coercion 转换的 trait object）
/// 由 `#[component]` on impl 块宏在编译期生成，已知具体类型和 trait，直接做类型转换。
pub type TraitObjAccessorFn = fn(Arc<dyn std::any::Any + Send + Sync>) -> Option<Arc<dyn Injectable>>;

/// 组件唯一标识符：由类型 ID 和 自定义名称组成
pub type ComponentKey = (TypeId, String);

/// 异步 Future 的装箱类型
///
/// 用于在 Trait 对象或动态分发中返回异步任务。
/// - `Pin<Box<...>>`: 也就是堆上的固定位置 Future。
/// - `Send`: 允许跨线程移动。
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// 组件创建函数签名
///
/// 闭包返回一个 BoxFuture，解析为组件实例 T。
pub type CreateFn<T> = Box<dyn FnOnce() -> BoxFuture<anyhow::Result<T>> + Send + Sync>;

/// 组件初始化函数签名
///
/// 接收组件的 Arc 引用，允许初始化逻辑中引用自身或进行异步操作。
pub type InitFn<T> = Box<dyn FnOnce(Arc<T>) -> BoxFuture<anyhow::Result<()>> + Send + Sync>;

/// 组件销毁函数签名
///
/// 接收组件的所有权（T），用于清理资源。
pub type DestroyFn<T> = Box<dyn FnOnce(T) -> BoxFuture<anyhow::Result<()>> + Send + Sync>;

/// Tokio 运行时工厂函数签名
/// 
/// 创建 Tokio 运行时实例。
pub type TokioRuntimeFactory = Box<dyn FnOnce() -> anyhow::Result<tokio::runtime::Runtime> + Send>;

/// 日志层工厂函数签名
/// 
/// 创建日志层实例。
pub type LogLayersFactory = Box<dyn FnOnce() -> Box<dyn Layer<Registry> + Send + Sync> + Send>;

/// 异步任务工厂函数签名
///
/// 接收取消令牌（CancellationToken），返回一个可执行的 Future。
/// 用于将后台任务注入到运行时中。
pub type TaskSpawnsFactory = Box<dyn FnOnce(CancellationToken) -> BoxFuture<anyhow::Result<()>> + Send>;
