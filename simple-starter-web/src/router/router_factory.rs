//! 路由工厂定义。
//!
//! 通过 `inventory` crate 实现编译期自动收集所有路由。
//! 每个使用路由宏（如 `#[get(...)]`）修饰的函数会生成一个 `RouteFactory` 实例，
//! 并在程序启动时被 `WebPlugin` 自动合并到主路由中。

use axum::Router;

/// 表示一个可延迟构建的路由单元。
///
/// - `router`: 一个无参函数，返回 `axum::Router`。
///   该函数在插件初始化时被调用，用于构建实际路由。
pub struct RouteFactory {
    pub router: fn() -> Router,
}

// 告诉 `inventory` crate：所有 `RouteFactory` 实例应在编译期被收集到全局静态集合中
inventory::collect!(RouteFactory);