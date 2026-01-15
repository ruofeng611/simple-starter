//! # 应用主入口与生命周期管理
//!
//! `Application` 是整个系统的运行时核心，提供：
//! - **插件系统**：支持依赖拓扑排序与初始化/关闭顺序控制
//! - **配置管理**：合并插件默认配置 + 用户 `application.toml` + profile 覆盖
//! - **组件容器**：通过 `#[auto_component]` 自动注册全局单例组件
//! - **定时任务**：基于 `tokio-cron-scheduler` 的后台/前台调度支持
//! - **双模式运行**：
//!   - **服务端模式**（默认）：监听 Ctrl+C/SIGTERM，适合 CLI/Service
//!   - **自定义主循环模式**：通过 `set_main_loop_hook` 接管控制流，适合 GUI/Web
//!
//! 调用 `.run()` 启动应用，执行完整的生命周期流程。

use crate::core::app_job::CronJob;
use crate::core::app_plugin::AppBasicPlugin;
use crate::global_state::COMPONENT_REPOSITORY;
use crate::loaders::component_loader::auto_collect_global_component_load;
use crate::loaders::config_loader::global_config_load;
use crate::{AppCoreUtil, Plugin};
use anyhow::anyhow;
use anyhow::{Context, Result};
use std::collections::{HashMap, VecDeque};
use std::thread::JoinHandle;
use tokio::runtime::Builder;
use tokio::sync::oneshot;
use tokio_cron_scheduler::{Job, JobScheduler};
use toml::{Table, Value};
use tracing::{debug, error, info};

/// 定时任务关闭句柄信息
pub(crate) struct CronJobShutdownHandleInfo {
    // === 在自定义主循环模式中，后台开启定时任务情况下关闭时需要用到的句柄 ===
    /// 发送停止信号给后台线程的通道
    pub(crate) bg_shutdown_tx: Option<oneshot::Sender<()>>,
    /// 后台线程句柄，用于等待线程结束
    pub(crate) bg_thread_handle: Option<JoinHandle<()>>,
}

impl CronJobShutdownHandleInfo {
    pub(crate) fn new() -> Self {
        CronJobShutdownHandleInfo {
            bg_shutdown_tx: None,
            bg_thread_handle: None,
        }
    }
}

/// 应用核心结构体
pub struct Application {
    /// 基础插件（必须存在，负责日志、配置等核心功能）
    basic_plugin: Box<dyn Plugin>,
    /// 用户注册的插件列表
    plugins: Vec<Box<dyn Plugin>>,
    /// 启动完成后执行的钩子函数（FnOnce）
    startup_hooks: Vec<Box<dyn FnOnce()>>,
    /// 应用关闭前执行的钩子函数（FnOnce）
    shutdown_hooks: Vec<Box<dyn FnOnce()>>,
    /// 主线程让出执行权后执行的函数
    main_loop_hook: Option<Box<dyn FnOnce(Application)>>,
    /// 定时任务关闭句柄信息
    cron_job_shutdown_handle: CronJobShutdownHandleInfo,
}

impl Application {
    /// 创建一个新的 `Application` 实例
    pub fn new() -> Self {
        Application {
            basic_plugin: Box::new(AppBasicPlugin::new()),
            plugins: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            main_loop_hook: None,
            cron_job_shutdown_handle: CronJobShutdownHandleInfo::new(),
        }
    }

    /// 注册一个插件（链式调用）
    ///
    /// # 示例
    /// ```rust,ignore
    /// Application::new()
    ///     .register_plugin(MyPlugin)
    ///     .run();
    /// ```
    pub fn register_plugin<T: Plugin + 'static>(mut self, plugin: T) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// 添加启动完成后的钩子函数
    ///
    /// # 示例
    /// ```rust,ignore
    /// .add_startup_hook(|| println!("App is ready!"))
    /// ```
    pub fn add_startup_hook<F>(mut self, hook: F) -> Self
    where
        F: FnOnce() + 'static,
    {
        self.startup_hooks.push(Box::new(hook));
        self
    }

    /// 添加应用关闭前的钩子函数（在插件 shutdown_hook 之前执行）
    ///
    /// # 示例
    /// ```rust,ignore
    /// .add_shutdown_hook(|| println!("Cleaning up before plugin shutdown..."))
    /// ```
    pub fn add_shutdown_hook<F>(mut self, hook: F) -> Self
    where
        F: FnOnce() + 'static,
    {
        self.shutdown_hooks.push(Box::new(hook));
        self
    }

    /// 设置创建主循环的钩子函数
    ///
    /// 钩子函数接收 `Application` **按值移动**（即 `self`），因此：
    /// - 你拥有整个 `Application` 实例的所有权；
    /// - 可以从中提取插件、执行逻辑，但之后无法再使用该 `Application`；
    /// - 此钩子执行后，`run()` 方法将结束（不会进入默认的信号等待循环）。
    ///
    /// # 示例
    /// ```rust,ignore
    /// Application::new()
    ///     .register_plugin(MyPlugin)
    ///     .set_main_loop_hook(|mut app| {
    ///         println!("Total plugins: {}", app.plugins.len());
    ///         // 注意：app 在此之后会被 drop
    ///     })
    ///     .run(); // 钩子在 run() 内部被调用
    /// ```
    pub fn set_main_loop_hook<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(Application) + 'static,
    {
        self.main_loop_hook = Some(Box::new(hook));
        self
    }

    /// 启动应用并执行完整生命周期
    ///
    /// 执行流程如下：
    /// 1. **配置阶段**：
    ///    - 收集所有插件的默认配置
    ///    - 加载 `application.toml` 并合并（含 profile 覆盖）
    /// 2. **初始化阶段**：
    ///    - 初始化 `AppBasicPlugin`（日志、配置等核心能力）
    ///    - 自动注册 `#[auto_component]` 标记的全局组件
    ///    - 对插件按依赖关系进行拓扑排序
    ///    - 依次初始化所有插件（按依赖顺序）
    ///    - 执行 `startup_hooks`
    /// 3. **运行阶段**（二选一）：
    ///    - **默认模式**（无 `main_loop_hook`）：
    ///      在当前线程启动 Tokio Runtime，监听退出信号，适合服务端/CLI。
    ///    - **自定义主循环模式**（有 `main_loop_hook`）：
    ///      将 `Application` 所有权移交钩子函数，由用户控制主循环，适合 GUI。
    ///    > 若存在 `CronJob`，会自动启动调度器（后台线程或当前 Runtime）。
    /// 4. **关闭阶段**：
    ///    - 触发退出信号（Ctrl+C / SIGTERM 或用户主动结束）
    ///    - 执行 `shutdown_hooks`
    ///    - 逆序调用插件的 `shutdown_hook`
    ///    - 清理全局组件仓库
    ///
    /// ⚠ 注意：若配置加载或插件初始化失败，程序将直接 `exit(1)`。
    pub fn run(mut self) {
        // 收集所有插件的默认配置（用于合并）
        let plugin_default_configs = self.collect_plugin_default_configs();

        // 加载用户配置（application.toml + profile）并与默认配置合并
        if let Err(e) = global_config_load(plugin_default_configs) {
            eprintln!("------------------------------------------------------------");
            eprintln!("   FATAL: Configuration Load Failed");
            eprintln!("   Error: {}", e);
            eprintln!("------------------------------------------------------------");
            std::process::exit(1);
        }

        // 初始化基础插件（设置日志等）
        match self.basic_plugin.init() {
            Ok(_) => {
                if self.basic_plugin.should_log_init() {
                    debug!("Core system (BasicPlugin) initialized.");
                }
            }
            Err(e) => {
                eprintln!(
                    "CRITICAL: Failed to initialize basic plugin (Logging/Config). Error: {}",
                    e
                );
                std::process::exit(1);
            }
        }

        // 自动收集并注册所有 #[auto_component] 标记的组件
        if let Err(e) = auto_collect_global_component_load() {
            error!(
                "Failed to auto-collect global components: {}. \
                This usually means a component was registered twice or failed to initialize.",
                e
            );
            std::process::exit(1);
        }

        // 按插件依赖关系进行拓扑排序
        if let Err(e) = self.sort_plugins_by_dependency() {
            error!("Failed to sort plugins by dependency: {}", e);
            std::process::exit(1);
        }

        // 依次初始化所有插件
        for plugin in self.plugins.iter_mut() {
            match plugin.init() {
                Ok(_) => {
                    if plugin.should_log_init() {
                        info!("Plugin initialized: [{}]", plugin.name());
                    }
                }
                Err(e) => {
                    error!("Plugin '{}' failed to initialize: {}", plugin.name(), e);
                    std::process::exit(1);
                }
            }
        }

        // 执行所有启动钩子
        if !self.startup_hooks.is_empty() {
            info!("Executing startup hooks...");
            for hook in self.startup_hooks.drain(..) {
                hook();
            }
            info!("Startup hooks completed.");
        }

        // === 启动运行阶段：根据是否存在自定义主循环选择执行模式 ===
        let has_cron_jobs = inventory::iter::<CronJob>.into_iter().next().is_some();
        if let Some(main_loop) = self.main_loop_hook.take() {
            // 自定义主循环模式（例如 GUI 应用）
            info!("Running user-defined main loop...");
            if has_cron_jobs {
                info!(mode = "custom_main_loop", cron_jobs = true, "Starting background scheduler thread");
                let (tx, rx) = oneshot::channel::<()>();
                self.cron_job_shutdown_handle.bg_shutdown_tx = Some(tx);
                // 启动后台线程
                self.cron_job_shutdown_handle.bg_thread_handle =
                    Some(std::thread::spawn(move || {
                        let rt = Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("Failed to create background Tokio runtime");
                        rt.block_on(async move {
                            let mut scheduler = create_scheduler().await;
                            info!("Background scheduler started.");
                            // 等待关闭信号
                            let _ = rx.await;
                            info!("Background scheduler received shutdown signal.");
                            scheduler
                                .shutdown()
                                .await
                                .expect("Failed to shutdown scheduler");
                        });
                    }));
            }
            main_loop(self);
        } else {
            // 默认服务端模式（CLI/Service）
            let rt = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create Tokio runtime");
            rt.block_on(async move {
                let mut local_scheduler: Option<JobScheduler> = None;
                if has_cron_jobs {
                    local_scheduler = Some(create_scheduler().await);
                }
                info!("Application started. Awaiting shutdown signal (Ctrl+C / SIGTERM)...");
                shutdown_signal().await;
                if let Some(mut scheduler) = local_scheduler.take() {
                    info!("Shutting down cron scheduler...");
                    scheduler
                        .shutdown()
                        .await
                        .expect("Failed to shutdown scheduler");
                    info!("Cron scheduler shutdown completed.");
                }
                self.shutdown();
            });
        }
    }

    /// 合并所有插件的默认配置
    fn collect_plugin_default_configs(&self) -> Value {
        let mut merged = Table::new();
        for plugin in &self.plugins {
            merge_plugin_default_config(&mut merged, plugin);
        }
        merge_plugin_default_config(&mut merged, &self.basic_plugin);
        Value::Table(merged)
    }

    /// 对插件进行拓扑排序（基于依赖关系）
    ///
    /// 使用 Kahn 算法检测环并排序。
    /// 若存在循环依赖或依赖未注册插件，则返回 Err。
    fn sort_plugins_by_dependency(&mut self) -> Result<()> {
        let mut plugin_map: HashMap<&'static str, Box<dyn Plugin>> = HashMap::new();
        let mut all_names = Vec::new();

        // 构建插件名到插件的映射，并检查重名
        for plugin in self.plugins.drain(..) {
            let name = plugin.name();
            if plugin_map.insert(name, plugin).is_some() {
                return Err(anyhow!(
                    "Duplicate plugin name: '{}'. Each plugin must have a unique name",
                    name
                ));
            }
            all_names.push(name);
        }

        // 构建依赖图和入度表
        let mut graph: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
        let mut in_degree: HashMap<&'static str, usize> = HashMap::new();
        for &name in &all_names {
            graph.entry(name).or_default();
            in_degree.entry(name).or_insert(0);
        }

        // 填充图：dep → [dependents]
        for &name in &all_names {
            let deps = plugin_map.get(name).unwrap().dependencies();
            for &dep in deps {
                if !plugin_map.contains_key(dep) {
                    return Err(anyhow!(
                        "Plugin '{}' depends on unregistered plugin '{}'. Make sure '{}' is registered first",
                        name,
                        dep,
                        dep
                    ));
                }
                graph.entry(dep).or_default().push(name);
                *in_degree.get_mut(&name).unwrap() += 1;
            }
        }

        // Kahn 算法：入度为 0 的节点入队
        let mut queue: VecDeque<&'static str> = VecDeque::new();
        for (&name, &deg) in &in_degree {
            if deg == 0 {
                queue.push_back(name);
            }
        }

        // 拓扑排序
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

        // 检测环
        if topo_order.len() != plugin_map.len() {
            let mut cycle_info = String::new();
            for &name in &all_names {
                if !topo_order.contains(&name) {
                    if !cycle_info.is_empty() {
                        cycle_info.push_str(", ");
                    }
                    cycle_info.push_str(name);
                }
            }
            return Err(anyhow!(
                "Circular dependency detected among plugins: [{}]. \
             Please check the dependency declarations in your plugins",
                cycle_info
            ));
        }

        // 按拓扑序重建插件列表
        self.plugins = topo_order
            .into_iter()
            .map(|name| {
                plugin_map
                    .remove(name)
                    .with_context(|| format!("Plugin missing during reorder: {}", name))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(())
    }

    /// 执行应用的优雅关闭流程
    ///
    /// 调用顺序：
    /// 1. 通知后台调度器线程退出（如适用）
    /// 2. 执行用户注册的 `shutdown_hooks`
    /// 3. 逆序调用插件的 `shutdown_hook`（先初始化的后关闭）
    /// 4. 关闭基础插件
    /// 5. 清空全局组件仓库（触发组件析构）
    ///
    /// 此方法通常由 `run()` 在退出信号后自动调用，
    /// 但在自定义主循环模式下，用户需自行调用 `app.shutdown()`。
    pub fn shutdown(&mut self) {
        // 关闭后台调度器线程（GUI/Custom Loop 模式）
        if let Some(tx) = self.cron_job_shutdown_handle.bg_shutdown_tx.take() {
            let _ = tx.send(()); // 发送停止信号
            info!("Shutting down background cron scheduler...");
        }
        if let Some(handle) = self.cron_job_shutdown_handle.bg_thread_handle.take() {
            if let Err(e) = handle.join() {
                error!("Failed to join background scheduler thread: {:?}", e);
            } else {
                info!("Background cron scheduler stopped.");
            }
        }

        // 执行用户自定义的 shutdown 钩子（在插件销毁前）
        if !self.shutdown_hooks.is_empty() {
            info!("Running user-defined shutdown hooks...");
            for hook in self.shutdown_hooks.drain(..) {
                hook();
            }
        }

        // 逆序执行所有插件的关闭钩子（先初始化的后关闭）
        for mut plugin in self.plugins.drain(..).rev() {
            if let Some(hook) = plugin.shutdown_hook() {
                hook();
                if plugin.should_log_init() {
                    info!("Plugin '{}' shut down", plugin.name());
                }
            }
        }

        // 关闭基础插件
        if let Some(hook) = self.basic_plugin.shutdown_hook() {
            hook();
        }

        // 清空全局组件仓库，触发组件析构
        COMPONENT_REPOSITORY.clear();

        info!("Application shutdown completed. Bye!");
    }
}

/// 等待退出信号（Ctrl+C 或 SIGTERM）
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
        _ = ctrl_c => {
            info!("Received SIGINT (Ctrl+C). Initiating graceful shutdown...");
        },
        _ = terminate => {
            info!("Received SIGTERM. Initiating graceful shutdown...");
        },
    }
}

async fn create_scheduler() -> JobScheduler {
    let scheduler = JobScheduler::new()
        .await
        .expect("Failed to create scheduler");

    // 遍历所有注册的 CronJob
    for job_desc in inventory::iter::<CronJob> {
        let name = job_desc.name;
        let cron_expr = job_desc.cron_expr;
        let runner = job_desc.runner;

        info!("Scheduling Job: [{}] schedule: '{}'", name, cron_expr);

        // 创建异步任务
        let job =
            Job::new_async(cron_expr, move |_uuid, _l| runner()).expect("Invalid cron expression");

        scheduler
            .add(job)
            .await
            .expect("Failed to add cron job to scheduler");
    }

    scheduler.start().await.expect("Failed to start scheduler");
    scheduler
}

/// 合并两个插件的默认配置（overlay 覆盖 base）
fn merge_plugin_default_config(merged: &mut Table, plugin: &Box<dyn Plugin>) {
    let default = plugin.default_config();
    if let Value::Table(plugin_table) = default {
        for (key, value) in plugin_table {
            match merged.get_mut(&key) {
                Some(existing) => {
                    // 递归合并
                    let new_val = AppCoreUtil::merge_toml_values(existing.clone(), value);
                    merged.insert(key, new_val);
                }
                None => {
                    merged.insert(key, value);
                }
            }
        }
    }
}
