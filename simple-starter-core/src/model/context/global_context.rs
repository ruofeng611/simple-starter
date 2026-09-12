//! # 全局上下文快照（对标 Java 静态 ApplicationContext 模式）
//!
//! 以两个全局静态槽位镜像 `AppContext` 的配置与组件容器（均为 `Arc` clone，
//! 非所有权转移），供**无上下文传播的场景**读取：用户自建后台线程、无法
//! 注入的工具函数等。
//!
//! 生命周期由组件加载流程维护（见 `component_loader`）：
//!
//! - **安装**：全部组件创建与初始化完成、容器冻结之后（`after_all_ready`
//!   批次前）写入并冻结快照——此后任意线程零锁读；
//! - **清空**：销毁前批次（`before_destroy`）执行完毕、销毁循环开始之前
//!   清空——此后读取返回 `None`（迟到读者安全失败，而非读到已掏空的容器）。
//!
//! # 合法使用窗口
//!
//! 快照仅在 **`after_all_ready` 与 `before_destroy` 之间**（即运行期）可读。
//! 窗口外读取返回 `None`。这是**依赖注入之外的逃生舱**，不是注入机制的
//! 替代：能通过字段注入 / 钩子参数拿到上下文的代码应优先使用注入。
//!
//! # 单实例约束
//!
//! 静态槽位在进程内全局唯一：同一进程先后启动多个 `Application` 时，
//! 第二次安装将失败（快照已冻结，`get_mut` 返回 `None` → panic，fail-fast）。
//! 与 Java 静态 ApplicationContext 同构（JVM 内同样只有一个槽位）。

use crate::model::component::ComponentContainer;
use crate::utils::freeze_cell::FreezeCell;
use std::sync::Arc;
use toml::Value;

/// 全局组件容器快照（安装一次、运行期零锁读、销毁前清空一次）
static GLOBAL_CONTAINER: FreezeCell<Arc<ComponentContainer>> = FreezeCell::empty();

/// 全局配置快照（安装一次、运行期零锁读、销毁前清空一次）
static GLOBAL_CONFIG: FreezeCell<Arc<Value>> = FreezeCell::empty();

/// 安装全局上下文快照（由组件加载流程在容器冻结后、`after_all_ready` 批次前调用）
///
/// 快照为 `Arc` clone：不转移所有权，`AppContext` 仍持有原值；
/// 已安装（二次启动或重复调用）时 panic（fail-fast，单实例约束）。
pub(crate) fn install_context_snapshot(container: &Arc<ComponentContainer>, config: Arc<Value>) {
    GLOBAL_CONTAINER.set(container.clone()).unwrap_or_else(|_| {
        panic!("Global container snapshot already installed (process-wide single Application constraint)")
    });
    GLOBAL_CONFIG.set(config).unwrap_or_else(|_| {
        panic!("Global config snapshot already installed (process-wide single Application constraint)")
    });
    GLOBAL_CONTAINER.freeze();
    GLOBAL_CONFIG.freeze();
}

/// 清空全局上下文快照（由组件加载流程在 `before_destroy` 批次后、销毁循环前调用）
///
/// 释放对容器与配置的 Arc 引用；此后 `app_container` / `app_config` 返回
/// `None`——销毁循环期间任何迟到读者安全失败，不会触达已掏空的容器。
pub(crate) fn clear_context_snapshot() {
    GLOBAL_CONTAINER.clear();
    GLOBAL_CONFIG.clear();
}

/// 读取全局组件容器快照（运行期合法窗口内可用，窗口外返回 `None`）
///
/// 返回借用而非 `Arc`：clone 决策点留给调用方（与 `AppContext::container()`
/// 返回 `&Arc` 的形态一致），外传容器 Arc 需自行负责其生命周期约束。
pub fn app_container() -> Option<&'static Arc<ComponentContainer>> {
    GLOBAL_CONTAINER.get()
}

/// 读取全局配置快照（运行期合法窗口内可用，窗口外返回 `None`）
pub fn app_config() -> Option<&'static Arc<Value>> {
    GLOBAL_CONFIG.get()
}
