use crate::core::app_config::{AppConfig, LoggerConfig, RuntimeConfig};
use crate::core::app_job::CronJob;
use crate::core::app_plugin::Plugin;
use crate::core::app_types::TaskFactory;
use crate::global_state::COMPONENT_REPOSITORY;
use crate::loaders::component_loader::{component_repository_load, shutdown_components};
use crate::loaders::config_loader::global_config_load;
use crate::utils::app_inner_util::{find_cycle_path, merge_toml_values};
use crate::{AppCoreUtil, BoxFuture, ComponentProcessorFactory, LogExpectExt};
use anyhow::{Context, anyhow};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::str::FromStr;
use std::thread::JoinHandle;
use tokio::runtime::Builder;
use tokio::task::JoinSet;
use tokio_cron_scheduler::{Job, JobScheduler};
use tokio_util::sync::CancellationToken;
use toml::{Value, toml};
use tracing::{debug, error, info, warn};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt, registry};

/// 应用程序构建器和运行时管理器
///
/// 负责生命周期管理：配置 -> 日志 -> 运行时 -> 组件 -> 插件 -> 任务 -> 退出清理。
pub struct Application {
    /// 异步任务工厂队列
    task_spawns: Vec<TaskFactory>,
    /// 用户注入的默认配置
    default_config: Vec<Value>,
    /// 注册的插件列表
    plugins: Vec<Box<dyn Plugin>>,
    /// 启动钩子
    startup_hooks: Vec<BoxFuture<()>>,
    /// 关闭钩子
    shutdown_hooks: Vec<BoxFuture<()>>,
    /// 自定义主循环钩子
    main_loop_hook: Option<Box<dyn FnOnce(Application) + Send>>,

    /// Tokio 运行时
    tokio_runtime: Option<tokio::runtime::Runtime>,
    /// 日志守卫 (必须持有以保证异步日志不丢失)
    log_guard: Option<WorkerGuard>,

    /// 后台线程句柄 (用于 GUI 模式等)
    background_handle: Option<JoinHandle<()>>,
    /// 全局取消令牌
    cancel_token: Option<CancellationToken>,
}

impl Application {
    /// 创建一个新的 Application 实例
    pub fn new() -> Self {
        Application {
            task_spawns: Vec::new(),
            default_config: Vec::new(),
            plugins: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            main_loop_hook: None,

            tokio_runtime: None,
            log_guard: None,

            background_handle: None,
            cancel_token: None,
        }
    }

    /// 注册一个异步任务，随应用启动
    pub fn register_task_spawn<F, Fut>(mut self, f: F) -> Self
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.task_spawns
            .push(Box::new(move |token| Box::pin(f(token))));
        self
    }

    /// 在运行时上下文中添加任务 (不推荐直接使用，仅供特定场景)
    pub fn add_task_spawn_in_runtime<F, Fut>(&mut self, f: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.task_spawns
            .push(Box::new(move |token| Box::pin(f(token))));
    }

    /// 添加默认配置 (优先级低于配置文件，但高于系统硬编码默认值)
    pub fn add_default_config(mut self, config: Value) -> Self {
        self.default_config.push(config);
        self
    }

    /// 注册插件
    pub fn register_plugin<T: Plugin + 'static>(mut self, plugin: T) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// 添加启动后立即执行的钩子
    pub fn add_startup_hook<F>(mut self, hook: F) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.startup_hooks.push(Box::pin(hook));
        self
    }

    /// 添加关闭前执行的钩子
    pub fn add_shutdown_hook<F>(mut self, hook: F) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.shutdown_hooks.push(Box::pin(hook));
        self
    }

    /// 设置自定义主循环 (例如用于 GUI 框架接管主线程)
    pub fn set_main_loop_hook<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(Application) + Send + 'static,
    {
        self.main_loop_hook = Some(Box::new(hook));
        self
    }

    /// 运行应用程序
    ///
    /// 包含完整的初始化和生命周期管理。
    pub fn run(mut self) {
        // 1. 加载全局配置
        if let Err(e) = global_config_load(self.get_merged_default_config()) {
            eprintln!("FATAL: Configuration Load Failed: {:?}", e);
            std::process::exit(1);
        }

        // 2. 初始化 Tracing 日志系统
        if let Err(e) = self.init_tracing() {
            eprintln!("FATAL: Tracing Init Failed: {:?}", e);
            std::process::exit(1);
        }

        // 3. 读取并打印关键配置信息
        let app_config: AppConfig =
            AppCoreUtil::get_config_to_struct("app").log_expect("Failed to get app config");

        if let Some(ref profile) = app_config.profile {
            info!("Active configuration profile: '{}'", profile);
        } else {
            debug!("Using default configuration (no profile activated)");
        }

        // 4. 初始化 Tokio 运行时
        if let Err(e) = self.init_runtime() {
            error!("Failed to initialize tokio runtime: {:?}", e);
            std::process::exit(1);
        }

        // 5. 加载组件仓库 (Inventory 模式)
        if inventory::iter::<ComponentProcessorFactory>
            .into_iter()
            .next()
            .is_some()
        {
            let handle = self.tokio_runtime.as_ref().unwrap().handle().clone();
            if let Err(e) = handle.block_on(component_repository_load()) {
                error!("Failed to load component repository: {:?}", e);
                std::process::exit(1);
            }
        }

        // 6. 初始化插件
        if !self.plugins.is_empty() {
            // 6.1 插件排序 (依赖检查)
            if let Err(e) = self.sort_plugins_by_dependency() {
                error!("Failed to sort plugins by dependency: {:?}", e);
                std::process::exit(1);
            }

            // 6.2 插件初始化
            if let Err(e) = self.init_plugins() {
                error!("Failed to initialize plugins: {:?}", e);
                std::process::exit(1);
            }
        }

        // 7. 执行启动钩子
        if !self.startup_hooks.is_empty() {
            let mut startup_hooks = std::mem::take(&mut self.startup_hooks);
            let handle = self.tokio_runtime.as_ref().unwrap().handle().clone();

            handle.block_on(async move {
                for hook in startup_hooks.drain(..) {
                    hook.await;
                }
            });
            info!("All startup hooks executed");
        }

        // 8. 启动运行阶段 (Main Loop)
        let has_cron_jobs = inventory::iter::<CronJob>.into_iter().next().is_some();

        if let Some(main_loop) = self.main_loop_hook.take() {
            // 模式 A: 自定义主循环 (GUI 等)
            self.run_custom_loop_mode(main_loop, has_cron_jobs);
        } else {
            // 模式 B: 默认主循环 (命令行/服务)
            self.run_default_loop_mode(has_cron_jobs, &app_config);
        }
    }

    /// 运行自定义主循环模式
    fn run_custom_loop_mode(
        mut self,
        main_loop: Box<dyn FnOnce(Application) + Send>,
        has_cron_jobs: bool,
    ) {
        info!("Running user defined main loop...");

        // 如果有后台任务，启动专用线程运行它们
        if has_cron_jobs || !self.task_spawns.is_empty() {
            self.cancel_token = Some(CancellationToken::new());
            let cancel_token = self.cancel_token.as_ref().unwrap().clone();
            let mut task_spawns = std::mem::take(&mut self.task_spawns);

            self.background_handle = Some(std::thread::spawn(move || {
                let rt = create_runtime().log_expect("Failed to create background runtime");
                rt.block_on(async move {
                    // 初始化调度器
                    let mut scheduler = if has_cron_jobs {
                        Some(create_scheduler().await)
                    } else {
                        None
                    };

                    // 启动普通任务
                    let mut task_set = if !task_spawns.is_empty() {
                        let mut set = JoinSet::new();
                        for factory in task_spawns.drain(..) {
                            set.spawn(factory(cancel_token.clone()));
                        }
                        Some(set)
                    } else {
                        None
                    };

                    info!("Background tasks started successfully.");

                    // 等待取消信号
                    cancel_token.cancelled().await;
                    info!("Background tasks received shutdown signal.");

                    // 优雅关闭调度器
                    if let Some(mut scheduler) = scheduler.take() {
                        if let Err(e) = scheduler.shutdown().await {
                            error!("Failed to shutdown scheduler: {:?}", e);
                        }
                    }

                    // 等待任务结束
                    if let Some(mut task_set) = task_set.take() {
                        while let Some(res) = task_set.join_next().await {
                            if let Err(e) = res {
                                error!("Task join error: {:?}", e);
                            } else if let Ok(Err(e)) = res {
                                error!("Task execution error: {:?}", e);
                            }
                        }
                    }
                })
            }));
        } else {
            warn!("No background tasks or cron jobs, skipping background runtime creation.");
        }

        // 移交控制权给用户
        main_loop(self);
    }

    /// 运行默认主循环模式
    fn run_default_loop_mode(mut self, has_cron_jobs: bool, app_config: &AppConfig) {
        let cancel_token = CancellationToken::new();
        let mut task_spawns = std::mem::take(&mut self.task_spawns);
        let rt = self.tokio_runtime.take().unwrap();

        rt.block_on(async move {
            let mut scheduler = if has_cron_jobs {
                Some(create_scheduler().await)
            } else {
                None
            };

            let mut task_set = if !task_spawns.is_empty() {
                let mut set = JoinSet::new();
                for factory in task_spawns.drain(..) {
                    set.spawn(factory(cancel_token.clone()));
                }
                Some(set)
            } else {
                None
            };

            info!(
                "Application {} started successfully.",
                app_config
                    .name
                    .as_ref()
                    .map(|s| format!("[{}] ", s))
                    .unwrap_or_default()
            );

            // 阻塞等待退出信号
            shutdown_signal().await;

            // 开始关闭流程，通知所有任务取消
            cancel_token.cancel();

            if let Some(mut scheduler) = scheduler.take() {
                if let Err(e) = scheduler.shutdown().await {
                    error!("Failed to shutdown scheduler: {:?}", e);
                }
            }

            if let Some(mut task_set) = task_set.take() {
                while let Some(res) = task_set.join_next().await {
                    if let Err(e) = res {
                        error!("Task join error: {:?}", e);
                    } else if let Ok(Err(e)) = res {
                        error!("Task execution error: {:?}", e);
                    }
                }
                info!("All tasks completed.");
            }

            // 调用内部关闭逻辑 (插件清理等)
            self.inner_shutdown().await;
        });
    }

    /// 获取系统硬编码的默认配置
    fn get_app_default_config(&self) -> Value {
        let default_config = toml! {
            [logger]
            level = "INFO"
            with_thread_id = true
            with_thread_name = true
            enable_console = true
            content_append = true

            [runtime]
            worker_thread_name = "tokio_worker"
        };
        Value::Table(default_config)
    }

    /// 获取合并后的默认配置
    ///
    /// 优先级 (低 -> 高):
    /// 1. 硬编码配置
    /// 2. 插件默认配置
    /// 3. 用户手动添加的默认配置
    fn get_merged_default_config(&mut self) -> Value {
        let mut final_config = self.get_app_default_config();

        for plugin in &self.plugins {
            let plugin_config = plugin.default_config();
            final_config = merge_toml_values(final_config, plugin_config);
        }

        for user_config in self.default_config.drain(..) {
            final_config = merge_toml_values(final_config, user_config);
        }

        final_config
    }

    /// 初始化日志系统
    fn init_tracing(&mut self) -> anyhow::Result<()> {
        let logger_config: LoggerConfig = AppCoreUtil::get_config_to_struct("logger")?;

        let log_level = tracing::Level::from_str(&logger_config.level)
            .map_err(|_| anyhow::anyhow!("Invalid log level: {}", &logger_config.level))?;

        let filter_layer = EnvFilter::builder()
            .with_default_directive(log_level.into()) // 如果没有 RUST_LOG，就用这个
            .from_env_lossy(); // 尝试读取 RUST_LOG，如果格式不对不报错，而是忽略环境变量

        let format_base = fmt::format()
            .with_thread_ids(logger_config.with_thread_id)
            .with_thread_names(logger_config.with_thread_name)
            .compact();

        // 控制台层
        let console_layer = if logger_config.enable_console {
            Some(
                fmt::layer()
                    .event_format(format_base.clone())
                    .with_writer(std::io::stdout),
            )
        } else {
            None
        };

        // 文件层
        let file_layer = if let Some(ref path_str) = logger_config.save_file {
            let path = Path::new(path_str);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .append(logger_config.content_append)
                .open(path)?;

            // 创建异步非阻塞 Writer
            let (non_blocking_writer, guard) = tracing_appender::non_blocking(file);
            // 保存 Guard 到 Application 实例中，让其在程序运行期间一直存在
            self.log_guard = Some(guard);

            Some(
                fmt::layer()
                    .event_format(format_base)
                    .with_writer(non_blocking_writer)
                    .with_ansi(false),
            )
        } else {
            None
        };

        let effective_level_str = filter_layer.to_string();

        // 注册所有 Layers
        registry()
            .with(filter_layer)
            .with(console_layer)
            .with(file_layer)
            .try_init()
            .map_err(|e| anyhow::anyhow!("Failed to init tracing: {:?}", e))?;

        //打印日志启动等级
        info!(
            "Log system initialized. Effective filter: {}",
            effective_level_str
        );

        Ok(())
    }

    /// 初始化 Tokio 运行时
    fn init_runtime(&mut self) -> anyhow::Result<()> {
        if self.main_loop_hook.is_some() {
            // 如果有自定义主循环，此处仅创建一个单线程运行时用于启动流程
            let rt = Builder::new_current_thread()
                .enable_all()
                .thread_name("main_starter_runtime")
                .build()
                .context("Failed to create current thread Tokio runtime")?;
            self.tokio_runtime = Some(rt);
        } else {
            self.tokio_runtime = Some(create_runtime()?);
        }
        Ok(())
    }

    /// 插件拓扑排序
    fn sort_plugins_by_dependency(&mut self) -> anyhow::Result<()> {
        let mut plugin_map: HashMap<&'static str, Box<dyn Plugin>> = HashMap::new();
        let mut all_names = Vec::new();

        // 1. 映射插件名并检查重复
        for plugin in self.plugins.drain(..) {
            let name = plugin.name();
            if plugin_map.insert(name, plugin).is_some() {
                return Err(anyhow!("Duplicate plugin name: '{}'", name));
            }
            all_names.push(name);
        }

        let mut graph: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        let mut in_degree: HashMap<&'static str, usize> = HashMap::new();

        for &name in &all_names {
            graph.entry(name).or_default();
            in_degree.entry(name).or_insert(0);
        }

        // 2. 构建依赖图
        for &name in &all_names {
            let deps = plugin_map.get(name).unwrap().dependencies();
            for &dep in deps {
                if !plugin_map.contains_key(dep) {
                    return Err(anyhow!(
                        "Plugin '{}' depends on unregistered plugin '{}'",
                        name,
                        dep
                    ));
                }
                graph.entry(dep).or_default().push(name);
                *in_degree.get_mut(&name).unwrap() += 1;
            }
        }

        // 3. 排序
        let mut queue: VecDeque<&'static str> = VecDeque::new();
        for (&name, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(name);
            }
        }

        let mut topo_order = Vec::new();
        while let Some(name) = queue.pop_front() {
            topo_order.push(name);
            if let Some(neighbors) = graph.get(name) {
                for &neighbor in neighbors {
                    let deg = in_degree.get_mut(&neighbor).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        // 4. 环检测
        if topo_order.len() != plugin_map.len() {
            let cycle_path = find_cycle_path(&graph, &in_degree);
            return Err(anyhow!(
                "Circular dependency detected in plugins: [{}]",
                cycle_path
            ));
        }

        // 5. 重组插件列表
        self.plugins = topo_order
            .into_iter()
            .map(|name| plugin_map.remove(name).unwrap())
            .collect();

        Ok(())
    }

    /// 初始化插件列表
    fn init_plugins(&mut self) -> anyhow::Result<()> {
        let mut plugins = std::mem::take(&mut self.plugins);
        let rt = self.tokio_runtime.as_ref().unwrap().handle().clone();

        // 这里的技巧是传入 &mut *self 来绕过借用检查
        let app = &mut *self;

        let result = rt.block_on(async move {
            for plugin in plugins.iter_mut() {
                match plugin.init(app).await {
                    Ok(_) => {
                        if plugin.should_log() {
                            info!("Plugin initialized: [{}]", plugin.name());
                        }
                    }
                    Err(e) => {
                        return Err(anyhow!("Plugin '{}' init failed: {:?}", plugin.name(), e));
                    }
                }
            }
            // 初始化成功后，必须把 plugins 返回出来，否则它就被丢弃了
            Ok(plugins)
        });

        match result {
            Ok(p) => {
                // 将初始化好的插件放回 self
                self.plugins = p;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// 内部关闭流程
    async fn inner_shutdown(&mut self) {
        // 取消令牌发起取消信息
        if let Some(cancel_token) = self.cancel_token.take() {
            cancel_token.cancel();
        }

        // 等待后台线程运行完毕，即所有异步任务执行完毕
        if let Some(background_handle) = self.background_handle.take() {
            if let Err(e) = background_handle.join() {
                error!("Failed to join background thread: {:?}", e);
            } else {
                info!("Background tasks thread stopped.");
            }
        }

        // 执行用户关闭钩子
        if !self.shutdown_hooks.is_empty() {
            let mut shutdown_hooks = std::mem::take(&mut self.shutdown_hooks);
            for hook in shutdown_hooks.drain(..) {
                hook.await;
            }
            info!("All shutdown hooks executed.")
        }

        // 逆序关闭插件
        if !self.plugins.is_empty() {
            let mut plugins = std::mem::take(&mut self.plugins);
            for mut plugin in plugins.drain(..).rev() {
                if let Err(e) = plugin.shutdown_hook().await {
                    error!("Plugin '{}' shutdown failed: {:?}", plugin.name(), e);
                } else if plugin.should_log() {
                    info!("Plugin shutdown: [{}]", plugin.name());
                }
            }
        }

        //如果组件仓库不为空，则销毁所有组件，这里判断组件仓库而不是收集的组件构造器是因为可能有运行时加进来的组件
        if !COMPONENT_REPOSITORY.is_empty() {
            if let Err(e) = shutdown_components().await {
                error!("Failed to destroy components: {:?}", e);
            }
        }

        info!("Application shutdown completed. Bye!");
    }

    /// 手动触发应用关闭
    pub fn shutdown(&mut self) {
        // 取出运行时并执行关闭
        if let Some(rt) = self.tokio_runtime.take() {
            rt.block_on(self.inner_shutdown());
        }
    }
}

/// 监听退出信号
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received SIGINT (Ctrl+C). Stopping..."),
        _ = terminate => info!("Received SIGTERM. Stopping..."),
    }
}

/// 创建调度器并加载任务
async fn create_scheduler() -> JobScheduler {
    let scheduler = JobScheduler::new()
        .await
        .log_expect("Failed to create scheduler");

    for job_desc in inventory::iter::<CronJob> {
        let name = job_desc.name;
        let cron_expr = job_desc.cron_expr;
        let runner = job_desc.runner;

        info!("Scheduling Job: [{}] expr: '{}'", name, cron_expr);

        let job = Job::new_async(cron_expr, move |_uuid, _l| runner())
            .log_expect(&format!("Invalid cron expression for job {}", name));

        scheduler
            .add(job)
            .await
            .log_expect("Failed to add cron job to scheduler");
    }

    scheduler
        .start()
        .await
        .log_expect("Failed to start scheduler");
    scheduler
}

/// 创建 Tokio 运行时
fn create_runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    let runtime_config: RuntimeConfig = AppCoreUtil::get_config_to_struct("runtime")?;
    let thread_name = runtime_config.worker_thread_name;

    let rt = match runtime_config.worker_thread_num {
        Some(1) => {
            info!("Using current thread Tokio runtime");
            Builder::new_current_thread()
                .enable_all()
                .thread_name(thread_name)
                .build()
                .context("Failed to create current thread runtime")?
        }
        Some(n) if n > 1 => {
            info!("Tokio runtime using {} worker threads", n);
            Builder::new_multi_thread()
                .worker_threads(n as usize)
                .enable_all()
                .thread_name(thread_name)
                .build()
                .context("Failed to create multi-thread runtime")?
        }
        Some(n) => return Err(anyhow!("worker_thread_num must be > 0, got: {}", n)),
        None => {
            info!("Using default multi-thread Tokio runtime");
            Builder::new_multi_thread()
                .enable_all()
                .thread_name(thread_name)
                .build()
                .context("Failed to create default runtime")?
        }
    };
    Ok(rt)
}
