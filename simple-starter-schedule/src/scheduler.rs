//! # 调度运行时
//!
//! 收尾期构建任务清单，运行期为每个任务启动独立驱动，关闭时按宽限期收编在飞任务体。

use crate::config::{CronConfig, JobConfig};
use crate::job::{CronJob, CronRunner};
use crate::trigger::Schedule;
use chrono::{FixedOffset, TimeDelta, Utc};
use simple_starter_core::ComponentContainer;
use simple_starter_core::anyhow;
use simple_starter_core::tracing::{debug, error, info, warn};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;

/// 等待分段上限
///
/// 等待基于单调时钟，系统时间跳变不会自动校正：分段重算把偏差限制在一个分段内。
const MAX_WAIT_SEGMENT: Duration = Duration::from_secs(60);

/// 待调度任务
pub(crate) struct ScheduledJob {
    /// 任务名
    pub(crate) name: &'static str,
    /// 触发规则
    pub(crate) schedule: Schedule,
    /// 任务体（已捕获目标实例）
    pub(crate) runner: CronRunner,
}

/// 从任务注册物构建任务清单
///
/// 触发规则按「配置覆盖优先，宏默认值兜底」合并；任务名重复、配置中出现未注册的
/// 任务名、触发规则非法、目标实例解析失败均直接返回错误，在启动期暴露。
pub(crate) fn build_jobs(
    container: &Arc<ComponentContainer>,
    offset: FixedOffset,
    config: &CronConfig,
) -> anyhow::Result<Vec<ScheduledJob>> {
    let descriptors: Vec<&'static CronJob> = inventory::iter::<CronJob>.into_iter().collect();

    // 1. 任务名唯一性：重名会导致配置键冲突与日志歧义
    let mut names = HashSet::new();
    for desc in &descriptors {
        if !names.insert(desc.name) {
            anyhow::bail!("Duplicate scheduled job name: '{}'", desc.name);
        }
    }

    // 2. 配置键必须命中已注册任务：拼错名字会让配置静默失效
    let unknown: Vec<&str> = config
        .configured_job_names()
        .into_iter()
        .filter(|name| !names.contains(name))
        .collect();
    if !unknown.is_empty() {
        let mut known: Vec<&str> = names.iter().copied().collect();
        known.sort_unstable();
        anyhow::bail!(
            "Unknown scheduled job(s) in `[cron.jobs]`: [{}]. Registered jobs: [{}]",
            unknown.join(", "),
            known.join(", ")
        );
    }

    // 3. 逐任务合并宏默认值与配置覆盖
    let mut jobs = Vec::new();
    for desc in descriptors {
        let job_config = config.job(desc.name);
        if job_config.is_some_and(|cfg| !cfg.enabled()) {
            info!("Scheduled job [{}] disabled by configuration", desc.name);
            continue;
        }

        let (expr, every) = match job_config.and_then(JobConfig::trigger_override) {
            Some((expr, every)) => (expr, every),
            None => (desc.default_expr, desc.default_every),
        };

        let schedule = Schedule::parse(expr, every, offset, desc.name)?;
        let runner = (desc.factory)(container)?;
        jobs.push(ScheduledJob {
            name: desc.name,
            schedule,
            runner,
        });
    }

    Ok(jobs)
}

/// 调度监督任务
///
/// 为每个任务启动独立驱动；收到关闭信号后等待驱动退出——驱动自身为在飞任务体
/// 保留宽限期，因此这里的等待时长上界为宽限期。
pub(crate) async fn supervise(
    jobs: Vec<ScheduledJob>,
    token: CancellationToken,
    grace: Duration,
) -> anyhow::Result<()> {
    let mut drivers = JoinSet::new();
    for job in jobs {
        drivers.spawn(drive(job, token.clone(), grace));
    }

    token.cancelled().await;

    while let Some(result) = drivers.join_next().await {
        if let Err(e) = result {
            error!("Scheduled job driver terminated abnormally: {:?}", e);
        }
    }
    info!("All scheduled jobs stopped.");

    Ok(())
}

/// 单任务驱动
///
/// 按触发规则循环执行；关闭信号到达后停止后续触发，并为在飞任务体保留宽限期。
async fn drive(job: ScheduledJob, token: CancellationToken, grace: Duration) {
    let mut next = job.schedule.next_after(Utc::now());

    while !token.is_cancelled() {
        let target = match next {
            Ok(target) => target,
            Err(e) => {
                error!("Scheduled job [{}] stopped: {:?}", job.name, e);
                return;
            }
        };

        // 等待到触发时刻，分段等待以限制系统时间跳变的影响
        loop {
            let remaining = target.signed_duration_since(Utc::now());
            if remaining <= TimeDelta::zero() {
                break;
            }
            let wait = remaining
                .to_std()
                .unwrap_or(Duration::ZERO)
                .min(MAX_WAIT_SEGMENT);
            tokio::select! {
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
        }

        debug!("Scheduled job [{}] triggered", job.name);

        // 任务体独立成任务：单次 panic 只影响本次执行，且关闭时可强制中止
        let mut handle = tokio::spawn((job.runner)());
        let cancelled = tokio::select! {
            result = &mut handle => {
                log_abnormal_end(job.name, result);
                false
            }
            _ = token.cancelled() => true,
        };

        if cancelled {
            // 关闭中：先给在飞任务体宽限期，超时后再强制中止
            match tokio::time::timeout(grace, &mut handle).await {
                Ok(result) => log_abnormal_end(job.name, result),
                Err(_) => {
                    warn!(
                        "Scheduled job [{}] exceeded the shutdown grace period; aborting",
                        job.name
                    );
                    handle.abort();
                    let _ = handle.await;
                }
            }
            return;
        }

        // 以上一次触发时刻为基准推进，避免执行耗时累积成漂移；若推进结果已过期
        // （执行耗时超过触发周期），直接跳到当前时刻之后，不补跑堆积
        let now = Utc::now();
        next = job.schedule.next_after(target).and_then(|candidate| {
            if candidate <= now {
                job.schedule.next_after(now)
            } else {
                Ok(candidate)
            }
        });
    }
}

/// 记录任务体的异常结束（panic 时打印 panic 信息）
fn log_abnormal_end(name: &str, result: Result<(), JoinError>) {
    let Err(e) = result else {
        return;
    };

    if e.is_panic() {
        let payload = e.into_panic();
        let message = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        error!("Scheduled job [{}] panicked: {}", name, message);
    } else {
        warn!("Scheduled job [{}] was cancelled", name);
    }
}
