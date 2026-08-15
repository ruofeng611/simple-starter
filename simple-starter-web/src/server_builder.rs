//! Web 服务构建器。
//!
//! 负责从配置和扩展注册表组装最终可运行的 Axum 服务。
//! 采用分层构建策略，确保各扩展点按正确顺序应用：
//!
//! 1. 合并自动收集和手动注册的路由
//! 2. 应用外部注册的路由修改器
//! 3. 挂载 `base_path`
//! 4. 应用外部注册的中间件（业务层，在 TraceLayer 之前）
//! 5. 应用框架自带的 TraceLayer（日志追踪）
//! 6. 构建监听器（支持自定义 TLS/UDS）
//! 7. 启动 `axum::serve`

use crate::config::web_config::WebConfig;
use crate::router::router_factory::RouteFactory;
use crate::web_extension::WebExtensionRegistry;
use axum::Router;
use simple_starter_core::anyhow::Context;
use simple_starter_core::tracing::{Level, info};
use simple_starter_core::{anyhow, AppCoreUtil};
use std::net::SocketAddr;
use tokio_util::sync::CancellationToken;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};

/// 构建并启动 Web 服务。
///
/// 该函数在应用核心后台任务中被调用，此时所有插件的 `assemble`、`components_ready` 与 `finalize` 已完成，
/// 扩展注册表已被完全填充。
pub(crate) async fn build_and_serve(
    web_config: WebConfig,
    manual_routers: Vec<Box<dyn FnOnce() -> Router + Send>>,
    mut registry: WebExtensionRegistry,
    cancel_token: CancellationToken,
) -> anyhow::Result<()> {
    // === 阶段 1: 构建基础路由 ===
    let mut router = Router::new();

    // 合并 inventory 自动收集的路由
    for route_factory in inventory::iter::<RouteFactory> {
        router = router.merge((route_factory.router)());
    }

    // 合并手动注册的路由
    for factory in manual_routers {
        router = router.merge(factory());
    }

    // === 阶段 2: 应用路由修改器 ===
    for modifier in registry.take_router_modifiers() {
        router = modifier(router);
    }

    // === 阶段 3: 应用 base_path ===
    let mut router = if let Some(ref base) = web_config.base_path {
        info!("Mounting routes under base path: {}", base);
        Router::new().nest(base, router)
    } else {
        router
    };

    // === 阶段 4: 应用外部中间件 ===
    for applier in registry.take_middleware_appliers() {
        router = applier(router);
    }

    // === 阶段 5: 应用框架日志追踪中间件 ===
    let log_level: Level = AppCoreUtil::get_config_value_by_path("logger.level")
        .context("Failed to load 'logger.level' config")?
        .as_str()
        .context("Failed to parse log level as string")?
        .parse()
        .context("Invalid log level format")?;

    router = router.layer(
        TraceLayer::new_for_http()
            .make_span_with(
                DefaultMakeSpan::new().include_headers(web_config.log_include_headers),
            )
            .on_request(DefaultOnRequest::new().level(log_level))
            .on_response(DefaultOnResponse::new().level(log_level)),
    );

    // === 阶段 6: 构建监听地址与监听器 ===
    let addr: SocketAddr = format!("{}:{}", web_config.binding, web_config.port)
        .parse()
        .with_context(|| {
            format!(
                "Invalid host:port configuration: {}:{}",
                web_config.binding, web_config.port
            )
        })?;

    // 监听器工厂由 WebPlugin 在收尾期从组件仓库获取后注入（默认直连绑定或用户覆盖实现）
    let factory = registry
        .take_listener_factory()
        .context("TcpListenerFactory component not registered")?;
    let listener = factory.bind(&web_config.binding, web_config.port).await?;

    let scheme = registry.server_scheme();
    info!("Server listening on {}://{}", scheme, addr);

    // === 阶段 7: 启动 Axum 服务 ===
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            cancel_token.cancelled().await;
            info!("Web server received shutdown signal.");
        })
        .await
        .context("Web server execution failed")?;

    Ok(())
}
