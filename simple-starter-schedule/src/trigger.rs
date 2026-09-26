//! # 触发规则
//!
//! 解析 cron 表达式或固定间隔，并按计算时区给出下一次触发时刻。

use chrono::{DateTime, FixedOffset, TimeDelta, Utc};
use croner::Cron;
use croner::parser::{CronParser, Seconds};
use simple_starter_core::anyhow;

/// 触发方式
enum Trigger {
    /// cron 表达式（秒级六段）
    Cron(Cron),
    /// 固定间隔
    Every(TimeDelta),
}

/// 触发规则：触发方式 + 计算时区
pub(crate) struct Schedule {
    trigger: Trigger,
    offset: FixedOffset,
}

impl Schedule {
    /// 解析触发规则
    ///
    /// cron 表达式与固定间隔必须且只能提供一个；表达式由 `job_name` 归属到具体任务，
    /// 便于启动期定位配置错误。
    pub(crate) fn parse(
        expr: Option<&str>,
        every: Option<&str>,
        offset: FixedOffset,
        job_name: &str,
    ) -> anyhow::Result<Self> {
        let trigger = match (expr, every) {
            (Some(_), Some(_)) => anyhow::bail!(
                "Scheduled job [{job_name}] declares both `expr` and `every`; exactly one is required"
            ),
            (None, None) => anyhow::bail!(
                "Scheduled job [{job_name}] declares neither `expr` nor `every`; exactly one is required"
            ),
            (Some(expr), None) => Trigger::Cron(
                CronParser::builder()
                    .seconds(Seconds::Required)
                    .dom_and_dow(true)
                    .build()
                    .parse(expr)
                    .map_err(|e| {
                        anyhow::anyhow!(
                            "Scheduled job [{job_name}]: invalid cron expression '{expr}': {e}"
                        )
                    })?,
            ),
            (None, Some(every)) => Trigger::Every(parse_interval(every).map_err(|e| {
                anyhow::anyhow!("Scheduled job [{job_name}]: invalid interval '{every}': {e}")
            })?),
        };

        Ok(Self { trigger, offset })
    }

    /// 计算 `from` 之后的首次触发时刻（严格晚于 `from`）
    pub(crate) fn next_after(&self, from: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
        match &self.trigger {
            Trigger::Cron(cron) => cron
                .find_next_occurrence(&from.with_timezone(&self.offset), false)
                .map(|time| time.with_timezone(&Utc))
                .map_err(|e| anyhow::anyhow!("Failed to compute next occurrence: {e}")),
            Trigger::Every(interval) => from
                .checked_add_signed(*interval)
                .ok_or_else(|| anyhow::anyhow!("Trigger time overflowed the supported range")),
        }
    }
}

/// 解析固定间隔
///
/// 支持 `s` / `m` / `h` / `d` 后缀的整数写法，如 `"30s"`、`"5m"`、`"2h"`。
fn parse_interval(text: &str) -> anyhow::Result<TimeDelta> {
    let text = text.trim();
    let (value, unit_seconds) = if let Some(value) = text.strip_suffix('s') {
        (value, 1_i64)
    } else if let Some(value) = text.strip_suffix('m') {
        (value, 60)
    } else if let Some(value) = text.strip_suffix('h') {
        (value, 3_600)
    } else if let Some(value) = text.strip_suffix('d') {
        (value, 86_400)
    } else {
        anyhow::bail!("expected `<n>s`, `<n>m`, `<n>h` or `<n>d`, e.g. \"30s\"");
    };

    let value: i64 = value
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("expected an integer before the unit, got '{value}'"))?;
    if value <= 0 {
        anyhow::bail!("interval must be greater than zero, got {value}");
    }

    let seconds = value
        .checked_mul(unit_seconds)
        .ok_or_else(|| anyhow::anyhow!("interval is too large"))?;
    TimeDelta::try_seconds(seconds).ok_or_else(|| anyhow::anyhow!("interval is too large"))
}
