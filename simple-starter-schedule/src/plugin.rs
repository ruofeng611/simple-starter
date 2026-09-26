//! # 定时任务插件

use crate::config::CronConfig;
use crate::scheduler;
use async_trait::async_trait;
use chrono::FixedOffset;
use simple_starter_core::tracing::info;
use simple_starter_core::{AppContext, Plugin, anyhow};
use toml::Value;

/// 定时任务插件
///
/// 收尾期读入 `[cron]` 配置与编译期注册的任务描述，构建任务清单并注册为后台任务；
/// 任务的启动与收编由框架统一负责。
pub struct SchedulePlugin;

impl SchedulePlugin {
    /// 创建插件实例
    pub fn new() -> Self {
        Self
    }
}

impl Default for SchedulePlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Plugin for SchedulePlugin {
    fn name(&self) -> &'static str {
        "SchedulePlugin"
    }

    /// `[cron]` 节缺省值：关闭宽限期 5 秒
    ///
    /// 触发计算时区取自 `[app] timezone`，不在本节提供缺省值。
    fn default_config(&self) -> Value {
        Value::Table(toml::toml! {
            [cron]
            shutdown_grace_ms = 5000
        })
    }

    /// 收尾期：构建任务清单并注册调度任务
    async fn finalize(&mut self, ctx: &mut AppContext) -> anyhow::Result<()> {
        let config: CronConfig = ctx.get_config_to_struct::<CronConfig>("cron")?;
        let offset = FixedOffset::east_opt(ctx.app_offset().whole_seconds())
            .ok_or_else(|| anyhow::anyhow!("App timezone offset is out of range"))?;
        let grace = config.shutdown_grace();

        let jobs = scheduler::build_jobs(ctx.container(), offset, &config)?;
        if jobs.is_empty() {
            info!("No scheduled jobs registered.");
            return Ok(());
        }
        for job in &jobs {
            info!("Scheduled job registered: [{}]", job.name);
        }

        ctx.add_task_spawn_factory_in_context(move |token| async move {
            scheduler::supervise(jobs, token, grace).await
        });

        Ok(())
    }
}
