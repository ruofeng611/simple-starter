use crate::core::app_config::{AppConfig, LoggerConfig, RuntimeConfig};
use crate::core::app_job::CronJob;
use crate::core::app_plugin::Plugin;
use crate::core::app_types::TaskSpawnsFactory;
use crate::global_state::COMPONENT_REPOSITORY;
use crate::loaders::component_loader::{component_repository_load, shutdown_components};
use crate::loaders::config_loader::global_config_load;
use crate::utils::app_inner_util::{find_cycle_path, merge_toml_values};
use crate::{
    AppCoreUtil, BoxFuture, ComponentProcessorFactory, LogExpectExt, LogLayersFactory,
    TokioRuntimeFactory,
};
use anyhow::{Context, anyhow};
use std::collections::{HashMap, VecDeque};
use std::str::FromStr;
use time::UtcOffset;
use time::macros::format_description;
use tokio::runtime::Builder;
use tokio::task::JoinSet;
use tokio_cron_scheduler::{Job, JobScheduler};
use tokio_util::sync::CancellationToken;
use toml::{Value, toml};
use tracing::{debug, error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt, registry};

/// 应用程序构建器和运行时管理器
///
/// 负责生命周期管理：配置 -> 日志 -> 运行时 -> 组件 -> 插件 -> 任务 -> 退出清理。
pub struct Application {
    /// 自定义的 Tokio 运行时的创建工厂
    tokio_runtime_factory: Option<TokioRuntimeFactory>,
    /// 自定义的日志 layer 创建工厂列表
    log_layers_factory: Vec<LogLayersFactory>,
    /// 异步任务创建工厂列表
    task_spawns_factory: Vec<TaskSpawnsFactory>,
    /// 用户添加的默认配置
    default_config: Vec<Value>,
    /// 注册的插件列表
    plugins: Vec<Box<dyn Plugin>>,
    /// 启动钩子
    startup_hooks: Vec<BoxFuture<anyhow::Result<()>>>,
    /// 关闭钩子
    shutdown_hooks: Vec<BoxFuture<anyhow::Result<()>>>,

    /// 自定义主循环钩子
    main_loop_hook: Option<Box<dyn FnOnce(Application) + Send>>,
    /// 异步任务的取消令牌
    cancel_token: Option<CancellationToken>,
    /// app核心管理任务句柄
    core_task_handle: Option<tokio::task::JoinHandle<()>>,

    /// Tokio 运行时
    tokio_runtime: Option<tokio::runtime::Runtime>,

    /// 日志守卫 (必须持有以保证异步日志不丢失)
    log_guard: Option<WorkerGuard>,
}

impl Application {
    /// 创建一个新的 Application 实例
    pub fn new() -> Self {
        Application {
            tokio_runtime_factory: None,
            log_layers_factory: Vec::new(),
            task_spawns_factory: Vec::new(),
            default_config: Vec::new(),
            plugins: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),

            main_loop_hook: None,
            cancel_token: None,
            core_task_handle: None,

            tokio_runtime: None,

            log_guard: None,
        }
    }

    /// 获取不可变的 Tokio 运行时引用
    pub fn get_runtime_as_ref(&self) -> &tokio::runtime::Runtime {
        self.tokio_runtime.as_ref().unwrap()
    }

    /// 获取可变的 Tokio 运行时引用
    pub fn get_runtime_as_mut(&mut self) -> &mut tokio::runtime::Runtime {
        self.tokio_runtime.as_mut().unwrap()
    }

    /// 获取存储的 Tokio 运行时的所有权
    pub fn take_runtime(&mut self) -> tokio::runtime::Runtime {
        self.tokio_runtime.take().unwrap()
    }

    /// 将 Tokio 运行时放到内部
    pub fn set_runtime(&mut self, runtime: tokio::runtime::Runtime) {
        self.tokio_runtime = Some(runtime);
    }

    /// 设置自定义的 Tokio 运行时的创建工厂
    pub fn set_tokio_runtime_factory<F>(mut self, factory: F) -> Self
    where
        F: FnOnce() -> anyhow::Result<tokio::runtime::Runtime> + Send + 'static,
    {
        self.tokio_runtime_factory = Some(Box::new(factory));
        self
    }

    /// 添加自定义的日志 layer 创建工厂
    pub fn add_log_layer_factory<L>(mut self, layer: L) -> Self
    where
        L: FnOnce() -> Box<dyn Layer<Registry> + Send + Sync> + Send + 'static,
    {
        self.log_layers_factory.push(Box::new(layer));
        self
    }

    /// 注册一个异步任务创建工厂，应用启动时创建对应任务提交到运行时中
    pub fn register_task_spawn_factory<F, Fut>(mut self, f: F) -> Self
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.task_spawns_factory
            .push(Box::new(move |token| Box::pin(f(token))));
        self
    }

    /// 在应用启动时的上下文中添加异步任务创建工厂(供插件使用)
    pub fn add_task_spawn_factory_in_context<F, Fut>(&mut self, f: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.task_spawns_factory
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
        F: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.startup_hooks.push(Box::pin(hook));
        self
    }

    /// 添加关闭前执行的钩子
    pub fn add_shutdown_hook<F>(mut self, hook: F) -> Self
    where
        F: Future<Output = anyhow::Result<()>> + Send + 'static,
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
            return;
        }

        // 2. 初始化 Tracing 日志系统
        if let Err(e) = self.init_tracing() {
            eprintln!("FATAL: Tracing Init Failed: {:?}", e);
            return;
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
            return;
        }

        // 5. 执行启动方法
        if let Err(e) = self.start() {
            error!("Failed to start application: {:?}", e);
            // 由于可能初始化了一些资源，这里调用shutdown方法清理已经创建的资源
            self.shutdown();
            return;
        }

        // 6. 启动运行阶段
        let cancel_token = CancellationToken::new();
        let task_spawns_factory = std::mem::take(&mut self.task_spawns_factory);
        let has_main_loop_hook = self.main_loop_hook.is_some();
        if let Some(main_loop_hook) = self.main_loop_hook.take() {
            if inventory::iter::<CronJob>.into_iter().next().is_some()
                || !task_spawns_factory.is_empty()
            {
                // 有自定义主循环，并且定时任务或者普通任务工厂不为空，将app核心管理任务作为一个普通的任务提交到运行时中
                let handle = self.tokio_runtime.as_ref().unwrap().handle().clone();
                let token_for_wait = cancel_token.clone();
                let core_task_handle = handle.spawn(app_core_task_spawn(
                    cancel_token.clone(),
                    app_config,
                    task_spawns_factory,
                    has_main_loop_hook,
                    async move {
                        token_for_wait.cancelled().await;
                    },
                ));
                // 保存cancel_token用于在shutdown时取消异步任务
                self.cancel_token = Some(cancel_token);
                // 保存core_task_handle用于在shutdown时等待其结束
                self.core_task_handle = Some(core_task_handle);
            }
            main_loop_hook(self);
            // 需要用户自己寻找时机调用shutdown方法
        } else {
            // 没有自定义主循环，将app核心管理任务作为主循环任务执行
            self.tokio_runtime
                .as_ref()
                .unwrap()
                .block_on(app_core_task_spawn(
                    cancel_token,
                    app_config,
                    task_spawns_factory,
                    has_main_loop_hook,
                    shutdown_signal(),
                ));
            // 核心任务结束执行清理操作
            self.shutdown();
        }
    }

    /// 获取系统硬编码的默认配置
    fn get_app_default_config(&self) -> Value {
        let default_config = toml! {
            [app]

            [logger]
            level = "INFO"

            enable_console = true

            with_thread_id = true
            with_thread_name = true

            file_name = "app.log"
            max_file_number = 30

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

        // 1. 基础 Filter, 对所有layer生效
        let log_level = tracing::Level::from_str(&logger_config.level)
            .map_err(|_| anyhow::anyhow!("Invalid log level: {}", &logger_config.level))?;
        let filter_layer = EnvFilter::builder()
            .with_default_directive(log_level.into()) // 如果没有 RUST_LOG，就用这个
            .from_env_lossy(); // 尝试读取 RUST_LOG，如果格式不对不报错，而是忽略环境变量

        // 2. 内置格式化器，其和内置控制台与文件输出绑定
        // 2.1. 定义格式
        let time_fmt = format_description!(
            "[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:6]"
        );
        // 2.2. 决定时区
        let offset = if let Some(tz_str) = &logger_config.timezone {
            // 如果配置了，尝试解析 "+08:00"
            UtcOffset::parse(
                tz_str,
                &format_description!("[offset_hour]:[offset_minute]"),
            )
            .context("Invalid timezone format (expect +HH:MM)")?
        } else {
            // 默认使用 UTC, 为了和文件输出时tracing-appender只能按照UTC创建文件匹配
            UtcOffset::UTC
        };

        // 2.3. 创建 Timer
        let timer = OffsetTime::new(offset, time_fmt);
        let format_base = fmt::format()
            .with_timer(timer)
            .with_thread_ids(logger_config.with_thread_id)
            .with_thread_names(logger_config.with_thread_name)
            .compact();

        // 3. 构建内置的控制台层
        let console_layer = if logger_config.enable_console {
            Some(
                fmt::layer()
                    .event_format(format_base.clone())
                    .with_writer(std::io::stdout),
            )
        } else {
            None
        };

        // 4. 构建内置的文件层 (按天滚动)
        let file_layer = if let Some(dir) = &logger_config.log_dir {
            // A. 确保目录存在
            std::fs::create_dir_all(dir).context("Failed to create log dir")?;

            // B. 配置按天滚动 (Daily Rolling)
            // 文件名为: <dir>/<name>.YYYY-MM-DD (例如 logs/app.log.2023-10-27)
            let file_appender = rolling::Builder::new()
                .rotation(rolling::Rotation::DAILY)
                .filename_prefix(&logger_config.file_name)
                .max_log_files(logger_config.max_file_number)
                .build(dir)
                .context("Failed to build file appender")?;

            // 创建异步非阻塞 Writer
            let (non_blocking_writer, guard) = tracing_appender::non_blocking(file_appender);
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

        // 5. 如果有自定义的日志层，则创建后添加
        let mut log_layers_vec: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::new();
        for log_layers_factory in self.log_layers_factory.drain(..) {
            log_layers_vec.push(log_layers_factory());
        }
        let log_layers_option = if log_layers_vec.is_empty() {
            None
        } else {
            Some(log_layers_vec)
        };
        registry()
            .with(log_layers_option)
            .with(file_layer)
            .with(console_layer)
            .with(filter_layer)
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
        if let Some(runtime_factory) = self.tokio_runtime_factory.take() {
            // 如果有自定义运行时工厂，则使用它创建运行时
            self.tokio_runtime = Some(runtime_factory()?);
            info!("Tokio runtime initialized");
        } else {
            // 否则根据配置创建运行时
            let runtime_config: RuntimeConfig = AppCoreUtil::get_config_to_struct("runtime")?;
            let thread_name = runtime_config.worker_thread_name;

            let rt = match runtime_config.worker_thread_num {
                Some(n) if n <= 0 => {
                    return Err(anyhow!("worker_thread_num must be > 0, got: {}", n));
                }
                // 当线程数为1并且没有自定义的主循环时创建单线程运行时
                Some(n) if n == 1 && self.main_loop_hook.is_none() => {
                    info!("Using current thread Tokio runtime");
                    Builder::new_current_thread()
                        .enable_all()
                        .thread_name(thread_name)
                        .build()
                        .context("Failed to create current thread runtime")?
                }
                Some(n) => {
                    info!("Tokio runtime using {} worker threads", n);
                    Builder::new_multi_thread()
                        .worker_threads(n as usize)
                        .enable_all()
                        .thread_name(thread_name)
                        .build()
                        .context("Failed to create multi thread runtime")?
                }
                None => {
                    info!("Using default multi thread Tokio runtime");
                    Builder::new_multi_thread()
                        .enable_all()
                        .thread_name(thread_name)
                        .build()
                        .context("Failed to create default runtime")?
                }
            };

            self.tokio_runtime = Some(rt);
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

        // 把 runtime 临时移出 self
        let rt = self.tokio_runtime.take().unwrap();

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

        // 把 runtime 放回去
        self.tokio_runtime = Some(rt);

        match result {
            Ok(p) => {
                // 将初始化好的插件放回 self
                self.plugins = p;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// 启动方法，加载相关组件仓库、插件、执行启动钩子
    fn start(&mut self) -> anyhow::Result<()> {
        // 1. 加载组件仓库 (Inventory 模式)
        if inventory::iter::<ComponentProcessorFactory>
            .into_iter()
            .next()
            .is_some()
        {
            let rt = self.tokio_runtime.as_ref().unwrap();
            rt.block_on(component_repository_load())?;
        }

        // 2. 初始化插件
        if !self.plugins.is_empty() {
            // 2.1 插件排序 (依赖检查)
            self.sort_plugins_by_dependency()?;
            // 2.2 插件初始化
            self.init_plugins()?;
        }

        // 3. 执行启动钩子
        if !self.startup_hooks.is_empty() {
            let mut startup_hooks = std::mem::take(&mut self.startup_hooks);
            let rt = self.tokio_runtime.as_ref().unwrap();
            rt.block_on(async move {
                for hook in startup_hooks.drain(..) {
                    if let Err(e) = hook.await {
                        return Err(anyhow!("Startup hook failed: {:?}", e));
                    }
                }
                Ok(())
            })?;
            info!("All startup hooks executed");
        }

        Ok(())
    }

    /// 关闭流程, 关闭流程中出现错误只打印错误不退出程序, 让每个关闭函数都能执行
    pub fn shutdown(&mut self) {
        if let Some(rt) = self.tokio_runtime.as_ref() {
            rt.block_on(async {
                // 发起异步任务取消指令
                if let Some(cancel_token) = self.cancel_token.take() {
                    cancel_token.cancel();
                }

                // 等待app核心管理任务完成
                if let Some(core_task_handle) = self.core_task_handle.take() {
                    if let Err(e) = core_task_handle.await {
                        error!(
                            "An error occurred in the core management tasks of the app: {:?}",
                            e
                        );
                    }
                }

                // 执行用户关闭钩子
                if !self.shutdown_hooks.is_empty() {
                    let mut shutdown_hooks = std::mem::take(&mut self.shutdown_hooks);
                    for hook in shutdown_hooks.drain(..) {
                        if let Err(e) = hook.await {
                            error!("Shutdown hook failed: {:?}", e);
                        }
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
            });
        }
    }
}

/// 创建调度器并加载任务
async fn create_scheduler() -> anyhow::Result<JobScheduler> {
    let scheduler = JobScheduler::new().await?;

    for job_desc in inventory::iter::<CronJob> {
        let name = job_desc.name;
        let cron_expr = job_desc.cron_expr;
        let runner = job_desc.runner;

        info!("Scheduling Job: [{}] expr: '{}'", name, cron_expr);

        let job = Job::new_async(cron_expr, move |_uuid, _l| runner())
            .context(format!("Invalid cron expression for job {}", name))?;

        scheduler.add(job).await?;
    }

    scheduler.start().await?;

    Ok(scheduler)
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

/// app核心管理任务，该任务提交注册的定时、普通任务到运行时中，并在收到关闭信号后等待所有异步任务取消完毕
async fn app_core_task_spawn<F>(
    cancel_token: CancellationToken,
    app_config: AppConfig,
    mut task_spawns_factory: Vec<TaskSpawnsFactory>,
    has_main_loop_hook: bool,
    // 阻塞的异步任务
    block_task: F,
) where
    F: Future<Output = ()> + Send + 'static,
{
    // 如果有定时任务，则创建一个定时任务管理器
    let mut scheduler = if inventory::iter::<CronJob>.into_iter().next().is_some() {
        match create_scheduler().await {
            Ok(scheduler) => Some(scheduler),
            Err(e) => {
                // 定时任务创建失败提前结束该任务
                error!("Failed to create scheduler: {:?}", e);
                return;
            }
        }
    } else {
        None
    };

    // 注册上来的普通任务不为空时，提交这些任务到运行时中
    let mut task_set = if !task_spawns_factory.is_empty() {
        let mut set = JoinSet::new();
        for factory in task_spawns_factory.drain(..) {
            set.spawn(factory(cancel_token.clone()));
        }
        Some(set)
    } else {
        None
    };

    info!(
        "Application {}started successfully.",
        app_config
            .name
            .as_ref()
            .map(|s| format!("[{}] ", s))
            .unwrap_or_default()
    );

    // 阻塞任务，这里会阻塞，直到收到退出信号或者被取消
    block_task.await;

    // 如果是有自定义主循环，那么取消指令由其发出，上面阻塞会释放，没有时说明是接收退出信号取消，这里在收到退出信号后发出取消信号关闭其他异步任务
    if !has_main_loop_hook {
        cancel_token.cancel();
    }

    // 关闭定时任务
    if let Some(mut scheduler) = scheduler.take() {
        if let Err(e) = scheduler.shutdown().await {
            error!("Failed to shutdown scheduler: {:?}", e);
        } else {
            info!("Scheduler shutdown completed.");
        }
    }

    // 普通任务内部自己处理了取消时机，即监听了取消令牌的取消信号，这里等待所有任务结束
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
}
