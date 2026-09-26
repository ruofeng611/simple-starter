# simple-starter-schedule

`simple-starter-schedule` 是 simple-starter 框架的**定时任务插件**：用宏声明任务，由插件在启动期装配，由框架托管运行与关闭。

- 任务是「注册物 + 任务体」：宏在编译期注册任务描述，插件在收尾期解析目标实例并构建任务体；
- 任务由框架的后台任务机制启动与收编，因此任务体可以安全持有组件实例；
- 触发规则可写在宏上作为默认值，也可由配置覆盖、按环境关闭；
- 需显式注册插件，否则任务不会被调度。

## 一、快速开始

```rust
use std::sync::Arc;
use simple_starter_core::{Application, component, tracing};
use simple_starter_schedule::{SchedulePlugin, cron_job, scheduled};

// 组件方法任务：任务体是组件方法，其依赖由组件在装配期完成注入
#[component]
pub struct HeartbeatService {
    #[inject]
    repository: Arc<Repository>, // 任意已注册组件
}

#[scheduled]
impl HeartbeatService {
    #[cron_job(every = "30s")]
    async fn tick(&self) {
        tracing::info!("心跳: {}", self.repository.name());
    }
}

// 自由函数任务：无参 async fn
#[cron_job("0 0 3 * * *")]
async fn daily_cleanup() {
    tracing::info!("每日清理");
}

fn main() {
    Application::new()
        .register_plugin(SchedulePlugin::new())
        .run();
}
```

## 二、任务形态

| 形态 | 声明方式 | 签名要求 | 依赖来源 |
|---|---|---|---|
| 组件方法 | `#[scheduled]` 标注组件 `impl` 块，块内方法加 `#[cron_job(...)]` | `async`、仅接收 `&self`、返回 `()` | 组件字段注入（装配期完成） |
| 自由函数 | `#[cron_job(...)]` 标注函数本身 | 无参 `async fn` | 无；需要读配置或组件时，开启 `[app] enable_global_snapshot` 后经 `app_config()` / `app_container()` 获取 |

**怎么选**：任务体需要依赖注入（业务服务、仓库、配置组件）时用组件方法形态；纯工具性动作（清理临时文件、打点上报）用自由函数形态即可。

签名不合规在编译期报错：`#[cron_job]` method must be `async` / must take `&self` / must return `()`。方法上写了 `#[cron_job]` 但所属 `impl` 块缺少 `#[scheduled]` 时，会提示 `requires #[scheduled] on the enclosing impl block`。

## 三、任务名与配置键

任务名同时充当**配置键**与日志标识：

| 形态 | 默认任务名 | 示例 |
|---|---|---|
| 组件方法 | `类型短名::方法名` | `HeartbeatService::tick` |
| 自由函数 | 函数名（不含模块路径） | `daily_cleanup` |

- 默认名可能撞名（两个模块下的同名自由函数，或 `a::Service::tick` 与 `b::Service::tick`）；重名会在启动期报错，用显式命名区分：

  ```rust
  #[cron_job(every = "30s", name = "heartbeat")]
  ```

- 同一个组件类型可以写多个 `#[scheduled] impl` 块，各块中带标记的方法都会注册。

## 四、触发规则

宏参数给出**默认值**，可留空交给配置：

| 写法 | 含义 |
|---|---|
| `#[cron_job("*/5 * * * * *")]` | cron 表达式：秒级六段（秒 分 时 日 月 周） |
| `#[cron_job(every = "30s")]` | 固定间隔：整数 + `s` / `m` / `h` / `d` |
| `#[cron_job]` | 触发规则全部来自配置 |

- cron 表达式按 `[app] timezone`（缺省 `local`）的时区解释；
- `expr` 与 `every` 互斥：宏上同时给出即编译报错；两者都未给出时必须由配置提供，否则启动报错；
- **首次触发不在启动瞬间**：`every = "30s"` 的任务在启动约 30 秒后首次执行，cron 任务对齐到下一个匹配时刻。

## 五、配置

任务相关的覆盖与关闭写在 `[cron]` 节；时区放在 `[app]`，与日志时间戳共用：

| 配置项 | 类型 | 缺省 | 说明 |
|---|---|---|---|
| `[app] timezone` | 字符串 | `local` | 触发计算与日志时间戳共用：`local`（本机时区）或 `+HH:MM` / `-HH:MM` |
| `[cron] shutdown_grace_ms` | 整数 | `5000` | 关闭时等待在飞任务结束的宽限期（毫秒），超时强制中止 |
| `[cron.jobs."<任务名>"]` · `expr` | 字符串 | — | 覆盖宏上的 cron 表达式 |
| `[cron.jobs."<任务名>"]` · `every` | 字符串 | — | 覆盖宏上的固定间隔 |
| `[cron.jobs."<任务名>"]` · `enabled` | 布尔 | `true` | 置 `false` 关闭该任务 |

```toml
[app]
timezone = "+08:00"

[cron]
shutdown_grace_ms = 5000

# 覆盖宏上的触发规则（整体替换，便于按环境调整频率）
[cron.jobs."HeartbeatService::tick"]
every = "10s"

# 关闭任务
[cron.jobs.daily_cleanup]
enabled = false
```

- **覆盖是整体替换**：配置给出 `expr` 或 `every` 之一，就完全采用配置的规则，不与宏默认值混用；只写 `enabled` 时触发规则沿用宏默认值。
- 生效优先级（由 core 的配置合并顺序保证）：宏默认值 < 插件默认配置 < 用户 `add_default_config` < `application.toml` < Profile 配置（如 `application-dev.toml`），Profile 分层天然生效。
- **启动期校验**（避免配置静默失效）：配置里出现未注册的任务名、非法表达式、或某个任务同时给出 `expr` 与 `every`，都会让应用启动失败并指出具体任务；错误信息中会列出已注册任务名。`enabled = false` 的任务不构建，也不校验其表达式。

## 六、运行语义

| 关注点 | 行为 |
|---|---|
| 触发推进 | 以上一次目标时刻为基准推进，长周期不累积漂移；某次执行耗时超过触发周期时，直接跳到当前时刻之后，**不重叠、不补跑** |
| 并发 | 同一任务串行执行：本轮任务体结束后才开始下一轮等待 |
| 异常隔离 | 每次触发在独立任务中执行，单次 panic 只记错误日志（含 panic 内容），后续周期照常 |
| 关闭收编 | 停止后续触发 → 为在飞任务体保留宽限期 → 超时强制中止；收编完成后才销毁组件，因此任务体持有的组件实例不会阻碍销毁 |

调度侧日志：

| 日志 | 级别 | 含义 |
|---|---|---|
| `Scheduled job registered: [name]` | INFO | 任务已构建（启动期） |
| `Scheduled job [name] disabled by configuration` | INFO | 被配置关闭，未构建 |
| `No scheduled jobs registered.` | INFO | 一个任务都没注册 |
| `Scheduled job [name] triggered` | DEBUG | 一轮触发开始 |
| `Scheduled job [name] panicked: …` | ERROR | 任务体 panic（已隔离） |
| `Scheduled job [name] stopped: …` | ERROR | 触发规则无法继续计算（如表达式永不匹配），该任务停止 |
| `Scheduled job [name] exceeded the shutdown grace period; aborting` | WARN | 关闭时在飞任务超过宽限期，被强制中止 |
| `All scheduled jobs stopped.` | INFO | 所有任务驱动已退出（关闭流程） |

调度侧不打印绝对触发时刻或倒计时——日志行的时间戳即事件时刻；任务体的业务日志由任务体自行打印。

## 七、约束与排障

- **必须注册插件**：未注册 `SchedulePlugin` 时任务不会被调度，启动日志中也不会出现任务清单。
- **组件方法任务要求目标类型已注册为组件**（`#[component]` / `#[provider]`），否则启动期报错。
- **避免在工作线程里同步阻塞**：任务体中的阻塞调用（`std::thread::sleep`、同步 IO、长 CPU 计算）会占住运行时工作线程，拖慢同线程上的其它任务与定时器；配置 `[runtime] worker_thread_num = 1` 时尤其明显。
- 常见报错：

| 报错 | 原因 |
|---|---|
| `Duplicate scheduled job name: '…'` | 任务名重复，用 `name = "..."` 区分 |
| `Unknown scheduled job(s) in [cron.jobs]: […]` | 配置里写了不存在的任务名（多为拼写错误），错误信息会列出已注册任务 |
| `invalid cron expression '…'` / `invalid interval '…'` | 触发规则不合法 |
| `declares both expr and every` / `declares neither expr nor every` | 触发规则给多了或一个都没给（宏上冲突表现为编译错误） |
