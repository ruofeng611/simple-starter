use crate::core::app_types::BoxFuture;
use inventory;

/// 定时任务结构体
///
/// 用于注册 Cron 表达式驱动的后台任务。
pub struct CronJob {
    /// 任务名称
    pub name: &'static str,
    /// Cron 表达式 (例如 "0 0 * * * *")
    pub cron_expr: &'static str,
    /// 任务执行逻辑，返回一个 BoxFuture
    pub runner: fn() -> BoxFuture<()>,
}

// 自动收集 CronJob
inventory::collect!(CronJob);
