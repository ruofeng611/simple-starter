//! 一次性写入 + 一次性清空的不可变快照单元（freeze-once cell）
//!
//! 针对"启动期集中构建、运行期只读、销毁期清空"的访问模式设计，
//! 读路径零锁（atomic load + 直接索引），写路径不计耗时（仅装配/销毁期各一次）：
//!
//! - **BUILDING 期**（装配期）：单线程读写（`get` / `get_mut` 均可用）
//! - **READY 期**（运行期）：零锁并发读（`get`），写被拒绝（`get_mut` 返回 `None`）
//! - **终止**：`take`（取回所有权）/ `clear`（drop 内容），发生在 shutdown 单线程时刻
//!
//! # Safety 契约
//!
//! 本类型以 unsafe 突破 Rust 借用规则（同一 `&self` 上产生 `&T` 与 `&mut T`），
//! 换取读路径零锁，安全性由以下不变量保证：
//!
//! 1. BUILDING 期仅装配线程单线程访问（容器在 load 完成前不对外暴露，
//!    读写交替无重叠）；
//! 2. READY 期值不可变（`freeze` 后无写者），并发读安全；
//! 3. `take` / `clear` 时无并发读者（shutdown 前框架已取消全部后台任务，
//!    与销毁阶段 `Arc::try_unwrap` 要求计数为 1 同构的外部契约）。

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU8, Ordering};

/// 装配期：允许可变访问（仅装配线程）
const BUILDING: u8 = 0;
/// 运行期：值不可变，并发读安全
const READY: u8 = 1;

pub(crate) struct FreezeCell<T> {
    /// 状态机：BUILDING ↔ READY（`take` 后回到 BUILDING 且值为 `None`）
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

    /// 读当前值（BUILDING 期单线程 / READY 期并发，均安全）
    ///
    /// SAFETY: 见类型文档的 Safety 契约。Acquire load 充当内存屏障：
    /// 与 `freeze` 的 Release 配对，保证 READY 期读者看到 BUILDING 期的全部写入；
    /// BUILDING 期装配线程自读自写，无需同步。
    pub(crate) fn get(&self) -> Option<&T> {
        let _ = self.state.load(Ordering::Acquire);
        // SAFETY: 值在 READY 期不可变；BUILDING 期仅装配线程单线程访问
        unsafe { (*self.value.get()).as_ref() }
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

    /// 取回所有权：READY → BUILDING 并取走值（幂等：二次调用返回 `None`）
    ///
    /// SAFETY: 取回要求无并发读者（shutdown 单线程时刻，后台任务已取消，
    /// 见类型文档契约 3）。
    pub(crate) fn take(&self) -> Option<T> {
        if self.state.swap(BUILDING, Ordering::Acquire) == READY {
            // SAFETY: 见方法文档
            unsafe { (*self.value.get()).take() }
        } else {
            None
        }
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
// BUILDING 期可变访问仅限装配线程，不跨线程。
unsafe impl<T: Send + Sync> Sync for FreezeCell<T> {}
