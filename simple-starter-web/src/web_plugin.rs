use crate::config::web_config::WebConfig;
use crate::web_extension::{TcpListenerFactory, WebExtensionRegistry};
use async_trait::async_trait;
use axum::Router;
use simple_starter_core::anyhow::Context;
use simple_starter_core::{AppCoreUtil, Application, Plugin};
use toml::Value;

/// Web 插件结构
///
/// 负责集成 Axum Web 框架，自动发现并注册路由，启动 HTTP 服务。
///
/// # 使用方式
/// 在 `Application` 上注册即可自动启动 HTTP 服务：
///
/// ```ignore
/// simple_starter_core::Application::new()
///     .register_plugin(WebPlugin::new())
///     .run();
/// ```
///
/// # 三阶段生命周期
///
/// - `assemble`: 创建 `WebExtensionRegistry` 并放入 `Application` 扩展上下文，
///   供其他依赖 WebPlugin 的插件注册中间件、路由修改器等扩展。
/// - `finalize`: 在所有插件装配与组件就绪完毕后，从组件仓库获取 [`TcpListenerFactory`]
///   并消费注册表，注册延迟构建的后台任务。
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

    /// 装配期
    ///
    /// 将 `WebPlugin` 自身的 `WebExtensionRegistry` 移入 `Application` 扩展上下文，
    /// 供其他依赖 WebPlugin 的插件继续注册扩展。
    async fn assemble(&mut self, ctx: &mut Application) -> simple_starter_core::anyhow::Result<()> {
        let registry = std::mem::take(&mut self.registry);
        ctx.insert_extension(registry);
        Ok(())
    }

    /// 收尾期
    ///
    /// 此时所有插件的 `assemble` 与 `components_ready` 已完成，扩展注册表已被填充
    /// （包含用户在 `new()` 阶段注册的扩展以及其他插件通过 `Application` 上下文注册的扩展）。
    /// 从组件仓库获取监听器工厂、消费注册表、注册后台任务。
    async fn finalize(
        &mut self,
        ctx: &mut Application,
    ) -> simple_starter_core::anyhow::Result<()> {
        // 1. 加载 Web 配置
        let web_config: WebConfig = AppCoreUtil::get_config_to_struct::<WebConfig>("web")
            .context("Failed to load 'web' config section")?;

        // 2. 取出手动注册的路由工厂
        let manual_router_factory = std::mem::take(&mut self.manual_router_factory);

        // 3. 从应用上下文中消费扩展注册表
        let mut registry = ctx
            .remove_extension::<WebExtensionRegistry>()
            .context("WebExtensionRegistry not found in application context")?;

        // 4. 从组件仓库获取监听器工厂（默认实现经条件注册保证存在，用户实现存在时自动退位）
        let listener_factory = AppCoreUtil::get_component_by_trait::<dyn TcpListenerFactory>()
            .context("TcpListenerFactory component not found in component repository")?;
        registry.set_listener_factory(listener_factory);

        // 5. 注册延迟构建的后台任务
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
