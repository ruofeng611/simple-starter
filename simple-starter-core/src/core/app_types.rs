use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{Layer, Registry};
use crate::core::app_component::TraitObjectEntry;

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
pub type ComponentKey = String;

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
