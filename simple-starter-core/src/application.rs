//! # 应用主入口与生命周期管理
//!
//! 定义 `Application` 结构体，负责：
//! - 插件注册与依赖排序
//! - 配置加载
//! - 组件自动收集
//! - 定时任务调度
//! - 启动钩子与优雅关闭

use crate::core::app_job::CronJob;
use crate::core::app_plugin::AppBasicPlugin;
use crate::loaders::component_loader::auto_collect_global_component_load;
use crate::loaders::config_loader::global_config_load;
use crate::{AppCoreUtil, Plugin};
use anyhow::{Context, Result};
use anyhow::anyhow;
use std::collections::{HashMap, VecDeque};
use tokio::runtime::Builder;
use tokio_cron_scheduler::{Job, JobScheduler};
use toml::{Table, Value};
use tracing::{error, info};

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
    /// 定时任务调度器（可选）
    scheduler: Option<JobScheduler>,
}

impl Application {
    /// 创建一个新的 `Application` 实例
    pub fn new() -> Self {
        Application {
            basic_plugin: Box::new(AppBasicPlugin::new()),
            plugins: Vec::new(),
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
            scheduler: None,
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

    /// 启动应用主流程
    ///
    /// 执行顺序：
    /// 1. 合并插件默认配置
    /// 2. 加载用户配置并合并
    /// 3. 初始化基础插件
    /// 4. 自动加载组件
    /// 5. 按依赖拓扑排序插件
    /// 6. 初始化所有插件
    /// 7. 执行 startup hooks
    /// 8. 启动定时任务 & 等待退出信号
    /// 9. 优雅关闭所有插件
    pub fn run(mut self) {
        // Step 1: 收集所有插件的默认配置（用于合并）
        let plugin_default_configs = self.collect_plugin_default_configs();

        // Step 2: 加载用户配置（application.toml + profile）并与默认配置合并
        if let Err(e) = global_config_load(plugin_default_configs) {
            println!("Failed to load global configuration: {}", e);
            // 提供配置文件的查找路径信息
            let config_dir =
                std::env::var("CONFIG_DIR").unwrap_or_else(|_| "./resources".to_string());
            println!("Configuration files are searched in: {}", config_dir);
            println!("Make sure 'application.toml' exists in the config directory");
            std::process::exit(1);
        }

        // Step 3: 初始化基础插件（设置日志等）
        match self.basic_plugin.init() {
            Ok(_) => {
                if self.basic_plugin.should_log_init() {
                    info!("Basic plugin initialized successfully");
                }
            }
            Err(e) => {
                panic!("Failed to init basic plugin: {}", e);
            }
        }

        // Step 4: 自动收集并注册所有 #[auto_component] 标记的组件
        if let Err(e) = auto_collect_global_component_load() {
            error!("Failed to auto-collect global components: {}", e);
            error!(
                "This may indicate duplicate component registration or component initialization issues"
            );
            std::process::exit(1);
        }

        // Step 5: 按插件依赖关系进行拓扑排序
        if let Err(e) = self.sort_plugins_by_dependency() {
            error!("Failed to sort plugins by dependency: {}", e);
            std::process::exit(1);
        }

        // Step 6: 依次初始化所有插件
        for plugin in self.plugins.iter_mut() {
            match plugin.init() {
                Ok(_) => {
                    if plugin.should_log_init() {
                        info!("Plugin '{}' initialized successfully", plugin.name());
                    }
                }
                Err(e) => {
                    error!("Plugin '{}' failed to initialize: {}", plugin.name(), e);
                    std::process::exit(1);
                }
            }
        }

        // Step 7: 执行所有启动钩子
        if !self.startup_hooks.is_empty() {
            info!("Running user defined startup hooks...");
            for hook in self.startup_hooks.drain(..) {
                hook(); // FnOnce，直接调用
            }
        }

        // Step 8: 启动 Tokio 运行时，运行主循环（含定时任务和信号监听）
        let rt = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");
        rt.block_on(self.run_main_loop());

        // Step 9: 执行用户自定义的 shutdown 钩子（在插件销毁前）
        if !self.shutdown_hooks.is_empty() {
            info!("Running user defined shutdown hooks...");
            for hook in self.shutdown_hooks.drain(..) {
                hook(); // FnOnce
            }
        }

        // Step 9.1: 逆序执行所有插件的关闭钩子（先初始化的后关闭，符合 RAII/依赖释放原则）
        for mut plugin in self.plugins.drain(..).rev() {
            if let Some(hook) = plugin.shutdown_hook() {
                hook();
                if plugin.should_log_init() {
                    info!("Plugin '{}' shutdown completed", plugin.name());
                }
            }
        }

        // Step 9.2: 关闭基础插件
        if let Some(hook) = self.basic_plugin.shutdown_hook() {
            hook();
        }

        info!("Application shutdown completed. All plugins terminated gracefully.");
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

    /// 主事件循环：启动定时任务 + 监听退出信号
    async fn run_main_loop(&mut self) {
        // 启动所有 cron job
        self.start_cron_jobs().await;

        info!("App started successfully.");

        // 等待 Ctrl+C 或 SIGTERM
        shutdown_signal().await;

        // 关闭调度器（如果存在）
        if let Some(mut scheduler) = self.scheduler.take() {
            info!("Shutting down all jobs gracefully...");
            scheduler
                .shutdown()
                .await
                .expect("Failed to shutdown scheduler");
            info!("All jobs terminated gracefully.");
        }
    }

    /// 启动所有通过 `#[cron_job]` 注册的定时任务
    async fn start_cron_jobs(&mut self) {
        // 检查是否有任何 CronJob 被注册
        if inventory::iter::<CronJob>.into_iter().next().is_some() {
            let scheduler = JobScheduler::new()
                .await
                .expect("Failed to create scheduler");

            // 遍历所有注册的 CronJob
            for job_desc in inventory::iter::<CronJob> {
                let name = job_desc.name;
                let cron_expr = job_desc.cron_expr;
                let runner = job_desc.runner;

                info!("Registering cron job: {} ({})", name, cron_expr);

                // 创建异步任务
                let job = Job::new_async(cron_expr, move |_uuid, _l| runner())
                    .expect("Invalid cron expression");

                scheduler
                    .add(job)
                    .await
                    .expect("Failed to add cron job to scheduler");
            }

            scheduler.start().await.expect("Failed to start scheduler");
            self.scheduler = Some(scheduler);
        }
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
