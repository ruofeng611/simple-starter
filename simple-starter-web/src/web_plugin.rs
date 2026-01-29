use crate::RouteFactory;
use crate::config::web_config::WebConfig;
use async_trait::async_trait;
use axum::Router;
use simple_starter_core::anyhow::Context;
use simple_starter_core::tracing::{Level, info};
use simple_starter_core::{AppCoreUtil, Application, Plugin};
use std::net::SocketAddr;
use toml::Value;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};

/// Web 插件结构
///
/// 负责集成 Axum Web 框架，自动发现并注册路由，启动 HTTP 服务。
pub struct WebPlugin {
    manual_router_factory: Vec<Box<dyn FnOnce() -> Router + Send>>,
}

impl WebPlugin {
    /// 创建 WebPlugin 实例
    pub fn new() -> Self {
        WebPlugin {
            manual_router_factory: Vec::new(),
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

    /// 插件初始化逻辑
    ///
    /// 包括配置加载、路由组装、中间件应用以及后台服务任务的启动。
    async fn init(&mut self, ctx: &mut Application) -> simple_starter_core::anyhow::Result<()> {
        // 1. 加载 Web 配置
        let web_config: WebConfig = AppCoreUtil::get_config_to_struct::<WebConfig>("web")
            .context("Failed to load 'web' config section")?;

        // 2. 构建主路由
        let mut main_router = Router::new();
        // 遍历自动收集到的路由工厂，逐个构建并合并
        for route_factory in inventory::iter::<RouteFactory> {
            let router = (route_factory.router)();
            main_router = main_router.merge(router);
        }
        // 添加手动注册的路由
        let manual_router_factory = std::mem::take(&mut self.manual_router_factory);
        for router_factory in manual_router_factory {
            main_router = main_router.merge(router_factory());
        }

        // 3. 应用 base_path 前缀（如果配置了）
        let mut app_router = if let Some(ref base) = web_config.base_path {
            info!("Mounting routes under base path: {}", base);
            Router::new().nest(base, main_router)
        } else {
            main_router
        };

        // 4. 配置并添加日志追踪中间件
        let log_level: Level = AppCoreUtil::get_config_value_by_path("logger.level")
            .context("Failed to load 'logger.level' config")?
            .as_str()
            .context("Failed to parse log level as string")?
            .parse()
            .context("Invalid log level format")?;

        app_router = app_router.layer(
            TraceLayer::new_for_http()
                .make_span_with(
                    DefaultMakeSpan::new().include_headers(web_config.log_include_headers),
                )
                .on_request(DefaultOnRequest::new().level(log_level))
                .on_response(DefaultOnResponse::new().level(log_level)),
        );

        // 5. 构建监听地址
        let addr: SocketAddr = format!("{}:{}", web_config.binding, web_config.port)
            .parse()
            .with_context(|| {
                format!(
                    "Invalid host:port configuration: {}:{}",
                    web_config.binding, web_config.port
                )
            })?;

        // 6. 将 Web 服务注册为应用的后台任务
        ctx.add_task_spawn_in_runtime(move |cancel_token| async move {
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .context("Failed to bind TCP listener")?;

            info!("Server listening on http://{}", addr);

            // 启动 Axum 服务，并绑定优雅退出信号
            axum::serve(listener, app_router)
                .with_graceful_shutdown(async move {
                    // 等待取消令牌被触发
                    cancel_token.cancelled().await;
                    info!("Web server received shutdown signal.");
                })
                .await
                .context("Web server execution failed")?;

            Ok(())
        });

        Ok(())
    }
}
