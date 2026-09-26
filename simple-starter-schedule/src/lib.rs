//! # 定时任务插件
//!
//! 以声明式方式注册定时任务并自主调度：
//!
//! - `#[cron_job]` 作用于无参 `async fn`，`#[scheduled]` 作用于组件 `impl` 块
//!   （块内带 `#[cron_job(...)]` 标记的方法成为任务体）；两者均在编译期注册任务描述，
//!   经 `inventory` 收集；
//! - 收尾期 [`SchedulePlugin`] 读入配置与任务描述，解析触发规则、解析目标组件实例，
//!   并把调度任务注册进应用的后台任务工厂；
//! - 调度任务由框架统一启动与收编（取消 → 宽限期 → 强制中止），任务体因此可
//!   安全持有组件实例，组件销毁前引用必然释放。
//!
//! 配置项：
//!
//! - `[app] timezone`：触发时刻计算时区（`local` 缺省，或 `+HH:MM` / `-HH:MM`），
//!   与日志时间戳共用同一偏移；
//! - `[cron] shutdown_grace_ms`：关闭时等待在飞任务结束的宽限期（毫秒），缺省 5000；
//! - `[cron.jobs."<任务名>"]`：单任务覆盖，`expr` / `every` 覆盖宏上写的触发规则
//!   （整体替换，不与宏默认值混用），`enabled = false` 关闭该任务。
//!
//! 触发规则以合并后的全局配置为准（profile 分层配置、用户默认配置均可覆盖）；
//! 未注册的任务名、非法触发规则、`expr` 与 `every` 同时给出都会在启动期报错。
//! 完整说明（任务形态、命名规则、运行语义、日志与排障）见 crate 的 README。

mod config;
mod job;
mod plugin;
mod scheduler;
mod trigger;

pub use job::{CronJob, CronRunner, CronRunnerFactory};
pub use plugin::SchedulePlugin;

// 定时任务宏重导出（依赖方无需直接依赖 simple-starter-macro）
pub use simple_starter_macro::{cron_job, scheduled};
