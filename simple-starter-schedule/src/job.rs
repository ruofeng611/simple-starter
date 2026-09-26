//! # 定时任务注册物
//!
//! 任务描述在编译期经 `inventory` 收集，收尾期由插件读入并构建调度任务。

use simple_starter_core::{BoxFuture, ComponentContainer, ComponentError};
use std::sync::Arc;

/// 单次触发的任务体
///
/// 构建期解析目标实例后捕获，触发期不再查询容器。
pub type CronRunner = Box<dyn Fn() -> BoxFuture<()> + Send + Sync>;

/// 任务体构建函数
///
/// 接收组件容器：需要组件实例的任务在此解析并捕获，解析失败向上传播，
/// 启动期即暴露；不依赖组件的任务忽略该参数。
pub type CronRunnerFactory = fn(&Arc<ComponentContainer>) -> Result<CronRunner, ComponentError>;

/// 定时任务注册物
pub struct CronJob {
    /// 任务唯一名称（兼作配置键）
    pub name: &'static str,
    /// cron 表达式默认值（秒级六段，如 `"*/5 * * * * *"`）
    pub default_expr: Option<&'static str>,
    /// 固定间隔默认值（如 `"30s"`）
    ///
    /// 与 [`CronJob::default_expr`] 互斥，二者必须且只能提供一个。
    pub default_every: Option<&'static str>,
    /// 任务体构建函数
    pub factory: CronRunnerFactory,
}

// 自动收集定时任务
inventory::collect!(CronJob);
