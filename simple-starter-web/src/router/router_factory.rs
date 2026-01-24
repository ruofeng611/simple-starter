//! 路由工厂定义。
//!
//! 利用 `inventory` crate 实现分布路由收集。
//! 使得分散在各个模块中的 Controller 可以自动注册到主路由中，解耦了路由定义与注册逻辑。

use axum::Router;

/// 路由工厂结构体
///
/// 包装了一个构建 `axum::Router` 的函数指针。
pub struct RouteFactory {
    /// 路由构建函数
    ///
    /// 此函数在应用启动阶段被调用，返回该模块对应的 Router 实例。
    pub router: fn() -> Router,
}

// 使用 inventory 宏进行收集
// 所有通过 inventory::submit! 提交的 RouteFactory 都会被收集到全局 registry 中。
inventory::collect!(RouteFactory);
