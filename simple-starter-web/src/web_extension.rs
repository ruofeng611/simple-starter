//! Web 插件扩展点定义。
//!
//! 提供 `WebExtensionRegistry`，用于收集其他插件注册的路由修改器、
//! 中间件和自定义监听器，在 Web 服务启动前统一消费。
//!
//! 该注册表通过 `Application::insert_extension` 放入应用上下文，
//! 实现插件间的解耦协作。

use axum::Router;
use simple_starter_core::anyhow;
use simple_starter_core::Injectable;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

/// 路由修改器类型别名。
///
/// 在所有路由合并完成后、`base_path` 应用前调用。
type RouterModifier = Box<dyn FnOnce(Router) -> Router + Send>;

/// 中间件应用器类型别名。
///
/// 通过闭包将 `tower::Layer` 应用到 Router，规避 Layer 泛型类型擦除问题。
type MiddlewareApplier = Box<dyn FnOnce(Router) -> Router + Send>;

/// TCP 监听器工厂。
///
/// 负责将监听地址与端口构建为 `TcpListener`。默认实现 [`DefaultTcpListenerFactory`] 为直连绑定，
/// 用户可通过注册组件提供 TLS/HTTPS、Unix Domain Socket 等自定义实现（自动覆盖默认实现）。
///
/// # 使用方式
/// 实现本 trait 并注册为组件，即可自动覆盖默认实现（默认实现带条件注册，用户提供实现时自动退位）：
///
/// ```ignore
/// #[simple_starter_core::component]
/// pub struct TlsListenerFactory;
///
/// #[simple_starter_core::injectable]
/// #[async_trait::async_trait]
/// impl TcpListenerFactory for TlsListenerFactory {
///     async fn bind(&self, host: &str, port: u16) -> simple_starter_core::anyhow::Result<TcpListener> {
///         // 在此构建 TLS / UDS 监听器
///         todo!()
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait TcpListenerFactory: Injectable {
    /// 按地址与端口构建监听器。
    async fn bind(&self, host: &str, port: u16) -> anyhow::Result<TcpListener>;
}

/// 默认 TCP 监听器工厂。
///
/// 直接绑定 TCP 监听。以条件注册方式参与组件装配：当用户未提供任何
/// [`TcpListenerFactory`] 实现时注册本默认实现，否则自动退位让位给用户实现。
#[simple_starter_macro::component(condition = simple_starter_core::ComponentCondition::on_missing_trait::<dyn TcpListenerFactory>())]
pub struct DefaultTcpListenerFactory;

#[simple_starter_macro::injectable]
#[async_trait::async_trait]
impl TcpListenerFactory for DefaultTcpListenerFactory {
    async fn bind(&self, host: &str, port: u16) -> anyhow::Result<TcpListener> {
        let addr: SocketAddr = format!("{}:{}", host, port)
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid host:port configuration: {}:{} ({})", host, port, e))?;
        TcpListener::bind(&addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind TCP listener: {}", e))
    }
}

/// Web 扩展注册表。
///
/// 持有所有由外部注册的扩展项，在 Web 服务启动前由 WebPlugin 统一消费。
pub struct WebExtensionRegistry {
    router_modifiers: Vec<RouterModifier>,
    middleware_appliers: Vec<MiddlewareApplier>,
    listener_factory: Option<Arc<dyn TcpListenerFactory>>,
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
    ///
    /// # 使用方式
    /// 通常在自定义插件的 `assemble` 阶段通过应用上下文获取注册表后调用：
    ///
    /// ```ignore
    /// ctx.get_extension_mut::<WebExtensionRegistry>()?
    ///     .add_router_modifier(|router| router.fallback(fallback_handler));
    /// ```
    pub fn add_router_modifier<F>(&mut self, modifier: F)
    where
        F: FnOnce(Router) -> Router + Send + 'static,
    {
        self.router_modifiers.push(Box::new(modifier));
    }

    /// 注册一个中间件应用器。
    ///
    /// 应用器会在 `base_path` 应用之后、框架自带的 `TraceLayer` 之前执行。
    ///
    /// # 使用方式
    /// 通常在自定义插件的 `assemble` 阶段通过应用上下文获取注册表后调用：
    ///
    /// ```ignore
    /// ctx.get_extension_mut::<WebExtensionRegistry>()?
    ///     .add_middleware(|router| router.layer(CompressionLayer::new()));
    /// ```
    pub fn add_middleware<F>(&mut self, applier: F)
    where
        F: FnOnce(Router) -> Router + Send + 'static,
    {
        self.middleware_appliers.push(Box::new(applier));
    }

    /// 注入监听器工厂组件。
    ///
    /// 由 `WebPlugin::finalize` 从组件仓库获取后注入（默认实现或用户覆盖）。
    pub(crate) fn set_listener_factory(&mut self, factory: Arc<dyn TcpListenerFactory>) {
        self.listener_factory = Some(factory);
    }

    pub(crate) fn take_router_modifiers(&mut self) -> Vec<RouterModifier> {
        std::mem::take(&mut self.router_modifiers)
    }

    pub(crate) fn take_middleware_appliers(&mut self) -> Vec<MiddlewareApplier> {
        std::mem::take(&mut self.middleware_appliers)
    }

    pub(crate) fn take_listener_factory(&mut self) -> Option<Arc<dyn TcpListenerFactory>> {
        self.listener_factory.take()
    }
}

impl Default for WebExtensionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
