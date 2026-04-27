use crate::config::web_config::WebConfig;
use crate::web_extension::WebExtensionRegistry;
use async_trait::async_trait;
use axum::Router;
use simple_starter_core::anyhow::Context;
use simple_starter_core::{AppCoreUtil, Application, Plugin};
use toml::Value;

/// Web 插件结构
///
/// 负责集成 Axum Web 框架，自动发现并注册路由，启动 HTTP 服务。
///
/// # 两阶段初始化
///
/// - `init`: 创建 `WebExtensionRegistry` 并放入 `Application` 扩展上下文，
///   供其他依赖 WebPlugin 的插件注册中间件、路由修改器等扩展。
/// - `post_init`: 在所有插件的 `init` 执行完毕后，消费注册表并构建/启动服务。
pub struct WebPlugin {
    manual_router_factory: Vec<Box<dyn FnOnce() -> Router + Send>>,
    registry: WebExtensionRegistry,
}

impl WebPlugin {
    /// 创建 WebPlugin 实例
    pub fn new() -> Self {
        WebPlugin {
            manual_router_factory: Vec::new(),
            registry: WebExtensionRegistry::new(),
        }
    }

    /// 手动添加额外路由
    ///
    /// 允许在自动收集之外，手动挂载动态构建的路由。
    pub fn add_manual_router_factory<F>(mut self, factory: F) -> Self
    where
        F: FnOnce() -> Router + Send + 'static,
    {
        self.manual_router_factory.push(Box::new(factory));
        self
    }

    /// 添加路由修改器（用户自定义扩展）
    ///
    /// 在 `WebPlugin::new()` 阶段即可注册，与其他插件通过 `Application` 上下文注册的效果相同。
    pub fn add_router_modifier<F>(mut self, modifier: F) -> Self
    where
        F: FnOnce(Router) -> Router + Send + 'static,
    {
        self.registry.add_router_modifier(modifier);
        self
    }

    /// 添加中间件（用户自定义扩展）
    ///
    /// 在 `WebPlugin::new()` 阶段即可注册，与其他插件通过 `Application` 上下文注册的效果相同。
    pub fn add_middleware<F>(mut self, applier: F) -> Self
    where
        F: FnOnce(Router) -> Router + Send + 'static,
    {
        self.registry.add_middleware(applier);
        self
    }

    /// 设置自定义监听器工厂（用户自定义扩展）
    ///
    /// 典型用途：实现 TLS/HTTPS、Unix Domain Socket 等。
    /// 设置后建议同时调用 `set_server_scheme("https")` 以修正日志输出。
    pub fn set_listener_factory<F, Fut>(mut self, factory: F) -> Self
    where
        F: FnOnce(&str, u16) -> Fut + Send + 'static,
        Fut: Future<Output = simple_starter_core::anyhow::Result<tokio::net::TcpListener>> + Send + 'static,
    {
        self.registry.set_listener_factory(factory);
        self
    }

    /// 设置服务协议前缀（影响启动日志）
    ///
    /// 默认值为 `"http"`，若启用了 TLS，建议设置为 `"https"`。
    pub fn set_server_scheme(mut self, scheme: impl Into<String>) -> Self {
        self.registry.set_server_scheme(scheme);
        self
    }
}

#[async_trait]
impl Plugin for WebPlugin {
    fn name(&self) -> &'static str {
        "WebPlugin"
    }

    fn default_config(&self) -> Value {
        let table = toml::toml! {
            [web]
            port = 8080
            binding = "0.0.0.0"
            log_include_headers = false
        };
        Value::Table(table)
    }

    /// 初始化阶段
    ///
    /// 将 `WebPlugin` 自身的 `WebExtensionRegistry` 移入 `Application` 扩展上下文，
    /// 供其他依赖 WebPlugin 的插件继续注册扩展。
    async fn init(&mut self, ctx: &mut Application) -> simple_starter_core::anyhow::Result<()> {
        let registry = std::mem::take(&mut self.registry);
        ctx.insert_extension(registry);
        Ok(())
    }

    /// 后置初始化阶段
    ///
    /// 此时所有插件的 `init` 已完成，扩展注册表已被填充（包含用户在 `new()` 阶段
    /// 注册的扩展以及其他插件通过 `Application` 上下文注册的扩展）。
    /// 消费注册表、构建 Router、注册后台任务。
    async fn post_init(
        &mut self,
        ctx: &mut Application,
    ) -> simple_starter_core::anyhow::Result<()> {
        // 1. 加载 Web 配置
        let web_config: WebConfig = AppCoreUtil::get_config_to_struct::<WebConfig>("web")
            .context("Failed to load 'web' config section")?;

        // 2. 取出手动注册的路由工厂
        let manual_router_factory = std::mem::take(&mut self.manual_router_factory);

        // 3. 从应用上下文中消费扩展注册表
        let registry = ctx
            .remove_extension::<WebExtensionRegistry>()
            .context("WebExtensionRegistry not found in application context")?;

        // 4. 注册延迟构建的后台任务
        ctx.add_task_spawn_factory_in_context(move |cancel_token| async move {
            crate::server_builder::build_and_serve(
                web_config,
                manual_router_factory,
                registry,
                cancel_token,
            )
            .await
        });

        Ok(())
    }
}
