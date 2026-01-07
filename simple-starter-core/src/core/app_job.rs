//! # 定时任务描述结构
//!
//! 通过 `inventory` crate 收集所有 `CronJob` 实例，
//! 供 `Application` 在启动时注册到调度器。

use inventory;
use std::future::Future;
use std::pin::Pin;

/// 定时任务描述
#[derive(Clone, Copy)]
pub struct CronJob {
    /// 任务名称（用于日志）
    pub name: &'static str,
    /// Cron 表达式（如 "0 0 * * *"）
    pub cron_expr: &'static str,
    /// 任务执行函数（返回 Future）
    pub runner: fn() -> Pin<Box<dyn Future<Output = ()> + Send>>,
}

// 通过 inventory 自动收集所有 CronJob 实例
inventory::collect!(CronJob);
