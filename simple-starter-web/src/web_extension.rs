//! Web 插件扩展点定义。
//!
//! 提供 `WebExtensionRegistry`，用于收集其他插件注册的路由修改器、
//! 中间件和自定义监听器，在 Web 服务启动前统一消费。
//!
//! 该注册表通过 `Application::insert_extension` 放入应用上下文，
//! 实现插件间的解耦协作。

use axum::Router;
use simple_starter_core::anyhow;

/// 路由修改器类型别名。
///
/// 在所有路由合并完成后、`base_path` 应用前调用。
type RouterModifier = Box<dyn FnOnce(Router) -> Router + Send>;

/// 中间件应用器类型别名。
///
/// 通过闭包将 `tower::Layer` 应用到 Router，规避 Layer 泛型类型擦除问题。
type MiddlewareApplier = Box<dyn FnOnce(Router) -> Router + Send>;

/// 监听器工厂类型别名。
///
/// 接收绑定地址和端口，返回异步构建的 TCP/TLS 监听器。
type ListenerFactory = Box<
    dyn FnOnce(&str, u16) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<tokio::net::TcpListener>> + Send>>
        + Send,
>;

/// Web 扩展注册表。
///
/// 持有所有由外部注册的扩展项，在 Web 服务启动前由 WebPlugin 统一消费。
pub struct WebExtensionRegistry {
    router_modifiers: Vec<RouterModifier>,
    middleware_appliers: Vec<MiddlewareApplier>,
    listener_factory: Option<ListenerFactory>,
    /// 服务协议前缀，用于日志输出（如 "http"、"https"）
    server_scheme: String,
}

impl WebExtensionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            router_modifiers: Vec::new(),
            middleware_appliers: Vec::new(),
            listener_factory: None,
            server_scheme: "http".to_string(),
        }
    }

    /// 设置服务协议前缀。
    ///
    /// 默认值为 `"http"`，若启用了 TLS/HTTPS，建议设置为 `"https"`。
    pub fn set_server_scheme(&mut self, scheme: impl Into<String>) {
        self.server_scheme = scheme.into();
    }

    pub(crate) fn server_scheme(&self) -> &str {
        &self.server_scheme
    }

    /// 注册一个路由修改器。
    ///
    /// 修改器会在所有路由合并完成后、`base_path` 应用前调用。
    pub fn add_router_modifier<F>(&mut self, modifier: F)
    where
        F: FnOnce(Router) -> Router + Send + 'static,
    {
        self.router_modifiers.push(Box::new(modifier));
    }

    /// 注册一个中间件应用器。
    ///
    /// 应用器会在 `base_path` 应用之后、框架自带的 `TraceLayer` 之前执行。
    pub fn add_middleware<F>(&mut self, applier: F)
    where
        F: FnOnce(Router) -> Router + Send + 'static,
    {
        self.middleware_appliers.push(Box::new(applier));
    }

    /// 设置自定义监听器工厂。
    ///
    /// 如果设置，将替代默认的 `tokio::net::TcpListener::bind`。
    /// 典型用途：实现 TLS/HTTPS、Unix Domain Socket 等。
    /// 注意：只能设置一次，后设置的会覆盖前者。
    pub fn set_listener_factory<F, Fut>(&mut self, factory: F)
    where
        F: FnOnce(&str, u16) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<tokio::net::TcpListener>> + Send + 'static,
    {
        self.listener_factory = Some(Box::new(move |host, port| Box::pin(factory(host, port))));
    }

    pub(crate) fn take_router_modifiers(&mut self) -> Vec<RouterModifier> {
        std::mem::take(&mut self.router_modifiers)
    }

    pub(crate) fn take_middleware_appliers(&mut self) -> Vec<MiddlewareApplier> {
        std::mem::take(&mut self.middleware_appliers)
    }

    pub(crate) fn take_listener_factory(&mut self) -> Option<ListenerFactory> {
        self.listener_factory.take()
    }
}

impl Default for WebExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
