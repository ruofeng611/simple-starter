//! # 插件系统接口与基础插件实现

use crate::AppCoreUtil;
use crate::core::app_config::{AppConfig, CoreConfig};
use anyhow::{Context, Result};
use toml::Value;
use tracing::info;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt;

/// 插件 trait
///
/// 所有插件必须实现此接口。
pub trait Plugin {
    /// 插件唯一名称
    fn name(&self) -> &'static str;

    /// 声明依赖的其他插件名称（用于拓扑排序）
    fn dependencies(&self) -> &[&'static str] {
        &[]
    }

    /// 提供默认配置（TOML 表）
    fn default_config(&self) -> Value {
        Value::Table(toml::value::Table::new())
    }

    /// 初始化插件（可访问全局配置）
    fn init(&mut self) -> Result<()>;

    /// 可选的关闭钩子（应用退出时调用）
    fn shutdown_hook(&mut self) -> Option<Box<dyn FnOnce()>> {
        None
    }

    /// 是否在初始化成功时打印日志
    fn should_log_init(&self) -> bool {
        true
    }
}

/// 基础插件：负责初始化日志系统和读取核心配置
pub(crate) struct AppBasicPlugin;

impl AppBasicPlugin {
    pub fn new() -> Self {
        AppBasicPlugin {}
    }
}

impl Plugin for AppBasicPlugin {
    fn name(&self) -> &'static str {
        "AppBasicPlugin"
    }

    /// 提供默认核心配置
    fn default_config(&self) -> Value {
        let config = AppConfig::default();
        Value::try_from(&config).unwrap()
    }

    /// 初始化 tracing 日志子系统
    fn init(&mut self) -> Result<()> {
        // 从配置中读取日志级别和 profile
        let core_config: CoreConfig = AppCoreUtil::get_config_to_struct("app")
            .context("Failed to load core configuration from 'app' section")?;
        let log_level: LevelFilter = (&core_config.log_level).parse()
            .with_context(|| format!(
            "Invalid log level '{}' in configuration. Valid values are: TRACE, DEBUG, INFO, WARN, ERROR",
            core_config.log_level
        ))?;

        // 初始化 tracing
        tracing_subscriber::fmt()
            .with_max_level(log_level)
            .with_writer(std::io::stderr)
            .event_format(
                fmt::format()
                    .with_thread_ids(core_config.with_thread_id)
                    .with_thread_names(core_config.with_thread_name),
            )
            .init();
        info!("Tracing subsystem initialized at level: {}", log_level);

        // 打印 profile 信息
        match &core_config.profile {
            Some(profile) => info!("Configuration profile activated: '{}'", profile),
            None => info!("No configuration profile specified, using default settings"),
        }

        Ok(())
    }

    /// 基础插件不打印初始化成功日志（避免冗余）
    fn should_log_init(&self) -> bool {
        false
    }
}
