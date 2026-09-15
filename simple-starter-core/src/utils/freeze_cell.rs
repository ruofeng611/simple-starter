//! 一次性写入 + 一次性清空的不可变快照单元（freeze-once cell）
//!
//! 针对“启动期集中构建、运行期只读、销毁期清空”的访问模式设计，
//! 读路径零锁（atomic load + 直接索引），写路径不计耗时（仅装配/销毁期各一次）。
//! 三态状态机：
//!
//! ```text
//! BUILDING ──freeze──> READY ──take(CAS)──> DESTROYING ──取走值──> BUILDING
//!   装配期                运行期               销毁期第一步              空态
//! ```
//!
//! - **BUILDING 期**（装配期）：仅装配任务可读写——`get` 经 poll 段守卫
//!   （`assembly_guard`）放行装配任务自身，外部并发读（含装配任务 spawn 的
//!   任务，即使同线程调度）返回 `None`；`get_mut` / `set` 仅此期可用（写侧
//!   对 core 外不可见）
//! - **READY 期**（运行期）：零锁并发读（`get`），写被拒绝（`get_mut` 返回 `None`）
//! - **DESTROYING 期**（销毁期第一步）：`take` 经 CAS 先转移至此态——
//!   此后发起的新读一律 `None`（物理拒绝并发读者）——再取走值；两步之间
//!   新读者或见 READY（值尚在，读安全）或见 DESTROYING（不触值），value 的
//!   读写竞争被状态机排除
//!
//! # Safety 契约
//!
//! 本类型以 unsafe 突破 Rust 借用规则（同一 `&self` 上产生 `&T` 与 `&mut T`），
//! 换取读路径零锁，安全性由以下不变量保证：
//!
//! 1. BUILDING 期仅装配任务访问（poll 段守卫运行时拒绝外部并发读；
//!    单写者：写侧 `pub(crate)` 且仅装配流程调用）；
//! 2. READY 期值不可变（`freeze` 后无写者），并发读安全；
//! 3. `take` 先 CAS 转移 DESTROYING 再取走值——**此后发起的新读**一律
//!    `None`；CAS 之前已跨过状态检查、正持有引用的读者窗口无法由状态机
//!    撤销——依赖销毁流程先收编全部托管任务（cancel + await，take 时无
//!    读者）；逃逸读者（未收编的后台任务/线程）与 take 的瞬间重叠是残留
//!    UB 面（小概率事件：查询入口内部立即 `Arc` clone，借用纳秒级，与
//!    take 窗口重叠极小，见 README 面 A）；`clear` 复用 `take`，同等保证。

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU8, Ordering};

/// 装配期：仅装配任务可读写（`get` 经 poll 段守卫放行装配任务自身）
const BUILDING: u8 = 0;
/// 运行期：值不可变，并发读安全
const READY: u8 = 1;
/// 销毁期第一步（`take` 经 CAS 转移）：`get` 一律返回 `None`，物理拒绝并发读者
const DESTROYING: u8 = 2;

pub(crate) struct FreezeCell<T> {
    /// 状态机：BUILDING → READY → DESTROYING（`take` 后回到 BUILDING 空态）
    state: AtomicU8,
    value: UnsafeCell<Option<T>>,
}

impl<T> FreezeCell<T> {
    /// 以空态创建（`value = None`），const 上下文可用：全局 static 声明
    /// 后于装配期经 `get_mut` 写入再 `freeze` 发布
    pub(crate) const fn empty() -> Self {
        Self {
            state: AtomicU8::new(BUILDING),
            value: UnsafeCell::new(None),
        }
    }

    /// 以 BUILDING 状态创建（装配期直接可变访问）
    pub(crate) fn new(value: T) -> Self {
        Self {
            state: AtomicU8::new(BUILDING),
            value: UnsafeCell::new(Some(value)),
        }
    }

    /// 读当前值（READY 期任意线程并发读；BUILDING 期仅装配任务；
    /// DESTROYING 期返回 `None`）
    ///
    /// 读安全：READY 期值不可变（并发读无竞争）；BUILDING 期经 poll 段
    /// 守卫放行装配任务自身、拒绝外部执行流；DESTROYING 期一律 `None`。
    ///
    /// SAFETY: 见类型文档的 Safety 契约。Acquire load 充当内存屏障：
    /// 与 `freeze` 的 Release 配对，保证 READY 期读者看到 BUILDING 期的全部写入。
    pub(crate) fn get(&self) -> Option<&T> {
        match self.state.load(Ordering::Acquire) {
            // READY 期值不可变：任意线程零锁读
            READY => unsafe { (*self.value.get()).as_ref() },
            // BUILDING 期仅装配任务可读：外部执行流（其他线程，或装配任务
            // spawn 的同线程调度任务）安全失败返回 `None`
            BUILDING if crate::utils::assembly_guard::current_is_assembly() => {
                unsafe { (*self.value.get()).as_ref() }
            }
            // DESTROYING / BUILDING（非装配任务）：安全失败
            _ => None,
        }
    }

    /// 可变访问（仅 BUILDING 期；`freeze` 后返回 `None`）
    ///
    /// SAFETY: BUILDING 期仅装配线程单线程访问，可变借用无竞争。
    pub(crate) fn get_mut(&self) -> Option<&mut T> {
        if self.state.load(Ordering::Acquire) == BUILDING {
            // SAFETY: 见方法文档
            unsafe { (*self.value.get()).as_mut() }
        } else {
            None
        }
    }

    /// 冻结：BUILDING → READY，发布装配期的全部写入，此后值不可变
    pub(crate) fn freeze(&self) {
        // Release：发布冻结前的全部写入，运行期读者经 Acquire 可见
        let old = self.state.swap(READY, Ordering::Release);
        debug_assert_eq!(old, BUILDING, "FreezeCell frozen twice");
    }

    /// 取回所有权：CAS READY → DESTROYING 并取走值（幂等：二次调用返回 `None`）
    ///
    /// CAS 先物理拒绝此后发起的新读（读到 DESTROYING 一律 `None`）再取走
    /// 值：两步之间新读者或见 READY（值尚在，读安全）或见 DESTROYING（不
    /// 触值），value 的读写竞争被状态机排除。
    ///
    /// 残余 UB 窗口：CAS 之前已 load READY、正夹在 `get` 内部（as_ref 与
    /// clone 之间）或正持有引用跨 await 的读者，无法被状态机撤销——该
    /// 窗口依赖销毁流程先收编全部托管任务（cancel + await，take 时无
    /// 读者）；逃逸读者（未收编的后台任务/线程）与 take 的瞬间重叠是
    /// 小概率残留面（查询入口内部立即 `Arc` clone，借用纳秒级，与 take
    /// 窗口重叠极小）。
    pub(crate) fn take(&self) -> Option<T> {
        // CAS 失败（BUILDING 空态 / DESTROYING 取走中）：幂等返回 None
        if self
            .state
            .compare_exchange(READY, DESTROYING, Ordering::Acquire, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        // SAFETY: 已转移至 DESTROYING（无读者），取走值安全
        let value = unsafe { (*self.value.get()).take() };
        // 回到 BUILDING 空态
        self.state.store(BUILDING, Ordering::Release);
        value
    }

    /// 写入或替换值（仅 BUILDING 期；`freeze` 后返回 `Err`）
    ///
    /// 与 `get_mut` 的区别：`get_mut` 只能访问已存在的值（空态返回 `None`），
    /// `set` 将空态写入或替换当前值——`empty` 创建的 static 槽位经此写入。
    ///
    /// SAFETY: BUILDING 期仅装配线程单线程访问，写入无竞争。
    pub(crate) fn set(&self, value: T) -> Result<(), T> {
        if self.state.load(Ordering::Acquire) == BUILDING {
            // SAFETY: 见方法文档
            unsafe {
                *self.value.get() = Some(value);
            }
            Ok(())
        } else {
            Err(value)
        }
    }

    /// 清空内容（`take` 并 drop；契约与 `take` 相同）
    pub(crate) fn clear(&self) {
        let _ = self.take();
    }
}

impl<T: Default> Default for FreezeCell<T> {
    /// 以 BUILDING 状态创建（装配期直接可变访问）
    fn default() -> Self {
        Self::new(T::default())
    }
}

// SAFETY: 见类型文档的 Safety 契约。READY 期值不可变，跨线程共享读安全；
// BUILDING 期读写仅限装配任务（poll 段守卫），不跨任务/线程共享可变访问。
unsafe impl<T: Send + Sync> Sync for FreezeCell<T> {}
