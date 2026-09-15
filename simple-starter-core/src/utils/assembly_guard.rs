//! 装配任务 poll 段守卫（三态 FreezeCell 的读闸门）
//!
//! 以 Future 包装器精确标记「装配任务正在 poll 的时间片」：
//!
//! - 进入 poll：将装配任务 id 写入线程局部槽位（栈式保存旧值）；
//! - 离开 poll（挂起/完成）：恢复旧值。
//!
//! `FreezeCell::get` 的 BUILDING 分支据此判断「当前执行流是否属于装配任务」：
//! - 装配任务自己的 poll 段（含 await 链内同步代码）→ 有值 → 放行；
//! - 其他任务的 poll 段（同线程调度的任务、其他线程）→ 无值 → 拒绝。
//!
//! 效益：TLS 有值的区间精确等于装配任务获得执行权的时间片——动态执行
//! 开关随任务调度自动开合：装配任务挂起期间任何读者被拒绝，spawn 任务
//! （即使与装配任务同线程调度）在各自 poll 段内读到无值槽位。
//! 顺带物理排除借用重叠 UB：装配任务 hold `&mut` 跨 await 期间（挂起时
//! 槽位已恢复），任何读者的 `get` 被拒绝 → 不存在与 `&mut` 重叠的 `&T`。

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

/// 装配任务 id 生成器（1 起始，0 保留为「无装配任务」）
static NEXT_ASSEMBLY_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// 当前线程正在 poll 的装配任务 id（0 = 无装配任务在执行）
    static ACTIVE_ASSEMBLY: Cell<u64> = const { Cell::new(0) };
}

/// 装配任务包装器：poll 段内标记装配 id，挂起/完成时恢复
pub(crate) struct AssemblyTask<F> {
    /// 本次装配的唯一 id（生成器分配，单实例契约下仅用于占位判断）
    id: u64,
    future: F,
}

impl<F: Future> AssemblyTask<F> {
    /// 包装装配期顶层 future（`Application::start` 的 block_on future）
    pub(crate) fn new(future: F) -> Self {
        AssemblyTask {
            id: NEXT_ASSEMBLY_ID.fetch_add(1, Ordering::Relaxed),
            future,
        }
    }
}

impl<F: Future> Future for AssemblyTask<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: 外层 AssemblyTask 已被 pin（block_on 内部处理），
        // poll 期间 `self.future` 地址不变——标准 pin 投影
        let this = unsafe { self.get_unchecked_mut() };
        // 进入装配任务 poll 段：栈式写入（保存旧值，支持嵌套与顺序多实例）
        let prev = ACTIVE_ASSEMBLY.replace(this.id);
        // SAFETY: 同上，future 在 poll 期间不被移动
        let result = unsafe { Pin::new_unchecked(&mut this.future) }.poll(cx);
        // 离开 poll 段（挂起或完成）：恢复旧值，此后任意读者被拒绝
        ACTIVE_ASSEMBLY.set(prev);
        result
    }
}

/// 当前执行流是否处于装配任务的 poll 段
///
/// `FreezeCell::get` 的 BUILDING 分支经此判断：装配任务自身可读未定型数据，
/// 其余执行流（外部线程 / 同线程调度的其他任务）返回 `None` 安全失败。
pub(crate) fn current_is_assembly() -> bool {
    ACTIVE_ASSEMBLY.with(|v| v.get() != 0)
}
