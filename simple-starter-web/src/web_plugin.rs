//! Web 插件实现。
//!
//! 负责：
//! 1. 在初始化阶段自动收集所有通过 `#[get]`/`#[post]` 等宏注册的路由；
//! 2. 构建主 `axum::Router`，支持嵌套 `base_path`；
//! 3. 添加 HTTP 请求/响应追踪日志；
//! 4. 启动独立线程运行 Tokio 异步服务器；
//! 5. 提供优雅关闭机制。

use crate::config::web_config::WebConfig;
use axum::Router;
use std::net::SocketAddr;
use std::thread::JoinHandle;
use simple_starter_core::{AppCoreUtil, Plugin};
use simple_starter_core::anyhow::Context;
use simple_starter_core::tracing::{error, info, Level};
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use toml::Value;
use tower_http::trace::{DefaultMakeSpan, DefaultOnRequest, DefaultOnResponse, TraceLayer};
use crate::RouteFactory;
use simple_starter_core::anyhow::Result;

/// Web 服务插件主结构体。
///
/// 管理服务器生命周期，包括启动、监听、关闭。
pub struct WebPlugin {
    shutdown_tx: Option<oneshot::Sender<()>>, // 用于触发优雅关闭
    server_thread: Option<JoinHandle<()>>,    // 服务器运行线程句柄
    main_router: Router,                      // 主路由（由宏自动注册的路由合并而成）
}

impl WebPlugin {
    /// 创建新插件实例。
    ///
    /// 此时会扫描所有通过 `inventory` 注册的 `RouteFactory`，
    /// 并将它们合并成一个初始的 `main_router`。
    pub fn new() -> Self {
        let mut main_router = Router::new();
        // 自动收集所有宏生成的路由工厂并合并
        for route_factory in inventory::iter::<RouteFactory> {
            let router = (route_factory.router)();
            main_router = main_router.merge(router);
        }
        Self {
            shutdown_tx: None,
            server_thread: None,
            main_router,
        }
    }

    /// 手动添加额外路由（链式调用）。
    ///
    /// 适用于非宏定义的动态路由或第三方中间件路由。
    pub fn add_route(mut self, router: Router) -> Self {
        self.main_router = self.main_router.merge(router);
        self
    }
}

impl Plugin for WebPlugin {
    /// 返回插件名称，用于日志和调试。
    fn name(&self) -> &'static str {
        "WebPlugin"
    }

    /// 提供默认配置模板（TOML 格式）。
    ///
    /// 当用户未提供 `web` 配置时，使用此默认值。
    fn default_config(&self) -> Value {
        let table = toml::toml! {
            [web]
            port = 8080
            binding = "0.0.0.0"
            worker_thread_name = "axum-web-worker"
            log_include_headers = false
        };
        Value::Table(table)
    }

    /// 初始化并启动 Web 服务器。
    ///
    /// 分步流程：
    /// 1. 从全局配置加载 `WebConfig`；
    /// 2. 若配置了 `base_path`，将主路由嵌套到该路径下；
    /// 3. 添加 `tower_http::TraceLayer` 日志中间件；
    /// 4. 解析监听地址（`binding:port`）；
    /// 5. 创建 `oneshot` 通道用于优雅关闭；
    /// 6. 在新线程中启动 Tokio 多线程运行时，并运行 `axum::serve`。
    fn init(&mut self) -> Result<()> {
        // 1. 加载配置
        let web_config: WebConfig = AppCoreUtil::get_config_to_struct::<WebConfig>("web")
            .context("Failed to load 'web' config section")?;

        // 2. 取出已构建的主路由
        let main_router = std::mem::take(&mut self.main_router);

        // 3. 应用 base_path（如有）
        let mut app_router = if let Some(ref base) = web_config.base_path {
            info!("Mounting routes under base path: {}", base);
            Router::new().nest(base, main_router)
        } else {
            main_router
        };

        // 4. 添加日志追踪中间件
        let log_level: Level = AppCoreUtil::get_config_value_by_path("app.log_level")
            .context("Failed to load 'app.log_level' config")?
            .as_str()
            .context("Failed to get log level as string")?
            .parse()
            .context("Invalid log level")?;
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
            .with_context(|| format!("Invalid host:port {}:{} in web config", web_config.binding, web_config.port))?;

        // 6. 创建关闭信号通道
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);

        // 7. 构建服务器异步任务
        let server_future = async move {
            let listener = tokio::net::TcpListener::bind(&addr)
                .await
                .expect("Cannot bind to address");
            info!("Server listening on http://{}", addr);
            axum::serve(listener, app_router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                    info!("Web server received shutdown signal.");
                })
                .await
                .expect("Server error");
        };

        // 8. 在新线程中启动 Tokio 运行时
        let handle = std::thread::Builder::new()
            .name("axum-web-starter".to_string())
            .spawn(move || {
                let mut rt_builder = Builder::new_multi_thread();
                // 可选：设置工作线程数量
                if let Some(num) = web_config.worker_thread_num {
                    if num > 0 {
                        rt_builder.worker_threads(num as usize);
                        info!("Using {} worker threads", num);
                    } else {
                        panic!("worker_thread_num must be > 0, got: {}", num);
                    }
                }
                let rt = rt_builder
                    .enable_all()
                    .thread_name(web_config.worker_thread_name)
                    .build()
                    .expect("Failed to build Tokio runtime");
                rt.block_on(server_future);
            })
            .expect("Failed to spawn axum web starter thread");

        self.server_thread = Some(handle);
        Ok(())
    }

    /// 注册关闭钩子。
    ///
    /// 在应用退出时：
    /// 1. 发送关闭信号；
    /// 2. 等待服务器线程完全退出；
    /// 3. 若线程 panic，记录错误。
    fn shutdown_hook(&mut self) -> Option<Box<dyn FnOnce()>> {
        let tx = self.shutdown_tx.take()?;
        let handle = self.server_thread.take()?;
        Some(Box::new(move || {
            let _ = tx.send(()); // 触发关闭
            if let Err(e) = handle.join() {
                error!("Web server thread panicked: {:?}", e);
            }
        }))
    }
}