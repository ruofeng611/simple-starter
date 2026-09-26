//! # 定时任务配置
//!
//! 对应 TOML 中的 `[cron]` 节：关闭宽限期与按任务名给出的触发规则覆盖
//! （`[cron.jobs."<任务名>"]`）。缺省值由插件默认配置提供；触发计算时区取自
//! `[app] timezone`，与日志时间戳共用同一偏移。

use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// 关闭宽限期默认值（毫秒）
const DEFAULT_SHUTDOWN_GRACE_MS: u64 = 5_000;

/// 定时任务配置
#[derive(Deserialize, Debug)]
pub(crate) struct CronConfig {
    /// 关闭时等待在飞任务结束的宽限期（毫秒），超时后强制中止
    pub(crate) shutdown_grace_ms: Option<u64>,
    /// 按任务名给出的触发规则覆盖
    pub(crate) jobs: Option<HashMap<String, JobConfig>>,
}

/// 单任务配置覆盖
#[derive(Deserialize, Debug)]
pub(crate) struct JobConfig {
    /// cron 表达式（覆盖宏上写的默认值）
    pub(crate) expr: Option<String>,
    /// 固定间隔（覆盖宏上写的默认值）
    pub(crate) every: Option<String>,
    /// 是否启用该任务，缺省启用
    pub(crate) enabled: Option<bool>,
}

impl CronConfig {
    /// 关闭宽限期
    pub(crate) fn shutdown_grace(&self) -> Duration {
        Duration::from_millis(self.shutdown_grace_ms.unwrap_or(DEFAULT_SHUTDOWN_GRACE_MS))
    }

    /// 查询任务的配置覆盖
    pub(crate) fn job(&self, name: &str) -> Option<&JobConfig> {
        self.jobs.as_ref()?.get(name)
    }

    /// 配置中出现的全部任务名（用于校验是否命中已注册任务）
    pub(crate) fn configured_job_names(&self) -> Vec<&str> {
        self.jobs
            .as_ref()
            .map(|jobs| jobs.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }
}

impl JobConfig {
    /// 是否启用（缺省启用）
    pub(crate) fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// 覆盖后的触发规则
    ///
    /// 未给出触发规则时返回 `None`，由宏上的默认值兜底；
    /// 覆盖按整体替换语义生效：只要给出 `expr` 或 `every` 之一，就完全采用配置的
    /// 规则，不与宏默认值混用。
    pub(crate) fn trigger_override(&self) -> Option<(Option<&str>, Option<&str>)> {
        if self.expr.is_none() && self.every.is_none() {
            return None;
        }
        Some((self.expr.as_deref(), self.every.as_deref()))
    }
}
