//! # 全局上下文快照（对标 Java 静态 ApplicationContext 模式）
//!
//! 以两个全局静态槽位镜像 `AppContext` 的配置与组件容器（均为 `Arc` clone，
//! 非所有权转移），供**无上下文传播的场景**读取：用户自建后台线程、无法
//! 注入的工具函数等。两个快照的安装与清空时机不同（窗口各异）：
//!
//! - **配置快照**：`Application::run` 配置加载完成后安装；组件销毁完成、
//!   关闭流程最后一步清空——窗口覆盖配置就绪到进程收尾的全周期；
//! - **组件容器快照**：`after_all_ready` 批次后安装（批次内回调经钩子参数
//!   `&Arc<ComponentContainer>` 访问，不依赖全局快照）；`before_destroy`
//!   批次前清空（批次内同理用钩子参数）——窗口仅覆盖运行期。
//!
//! 两快照均安装一次（写入后冻结，此后任意线程零锁读）、清空一次：
//! 清空后读取返回 `None`；此前已读取并持有 `Arc` clone 的读者在清空后
//! 访问的是内部已掏空的空壳（查询安全失败，不会悬垂）。
//!
//! # 合法使用窗口
//!
//! 快照是**依赖注入之外的逃生舱**，不是注入机制的替代：能通过字段注入 /
//! 钩子参数拿到上下文的代码应优先使用注入。
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

/// 全局组件容器快照（安装一次、运行期零锁读、销毁批次前清空一次）
static GLOBAL_CONTAINER: FreezeCell<Arc<ComponentContainer>> = FreezeCell::empty();

/// 全局配置快照（安装一次、零锁读、组件销毁完成后清空一次）
static GLOBAL_CONFIG: FreezeCell<Arc<Value>> = FreezeCell::empty();

/// 安装全局组件容器快照（由组件加载流程在 `after_all_ready` 批次后调用）
///
/// 快照为 `Arc` clone：不转移所有权，`AppContext` 仍持有原值；
/// 已安装（二次启动或重复调用）时 panic（fail-fast，单实例约束）。
pub(crate) fn install_container_snapshot(container: &Arc<ComponentContainer>) {
    GLOBAL_CONTAINER.set(container.clone()).unwrap_or_else(|_| {
        panic!("Global container snapshot already installed (process-wide single Application constraint)")
    });
    GLOBAL_CONTAINER.freeze();
}

/// 安装全局配置快照（由 `Application::run` 在配置加载完成后调用）
///
/// 快照为 `Arc` clone：不转移所有权，`AppContext` 仍持有原值；
/// 已安装（二次启动或重复调用）时 panic（fail-fast，单实例约束）。
pub(crate) fn install_config_snapshot(config: &Arc<Value>) {
    GLOBAL_CONFIG.set(config.clone()).unwrap_or_else(|_| {
        panic!("Global config snapshot already installed (process-wide single Application constraint)")
    });
    GLOBAL_CONFIG.freeze();
}

/// 清空全局组件容器快照（由组件加载流程在 `before_destroy` 批次前调用）
///
/// 释放对容器的 Arc 引用；此后 `app_container` 返回 `None`——批次回调经
/// 钩子参数访问容器，运行期迟到读者安全失败，不会触达已掏空的容器。
pub(crate) fn clear_container_snapshot() {
    GLOBAL_CONTAINER.clear();
}

/// 清空全局配置快照（由 `Application` 关闭流程在组件销毁完成后、关闭流程
/// 最后一步调用）
///
/// 此后 `app_config` 返回 `None`；此前已持有 `Arc` clone 的读者仍可继续
/// 使用配置数据（保活，安全）。
pub(crate) fn clear_config_snapshot() {
    GLOBAL_CONFIG.clear();
}

/// 读取全局组件容器快照（运行期窗口内可用：`after_all_ready` 批次后至
/// `before_destroy` 批次前；窗口外返回 `None`——批次内经钩子参数访问）
///
/// 返回 `Arc` clone（强引用）：保活容器对象。即使快照已清空（销毁期），
/// 持有者访问到的也只是内部已掏空的容器空壳——查询安全失败返回错误，
/// 不会悬垂；代价是持有者长期保存将滞留空壳内存（repository 已被 take）。
pub fn app_container() -> Option<Arc<ComponentContainer>> {
    GLOBAL_CONTAINER.get().cloned()
}

/// 读取全局配置快照（配置加载完成后至组件销毁完成前可用，窗口外返回 `None`）
///
/// 返回 `Arc` clone（强引用）：保活配置对象，语义同 `app_container`。
pub fn app_config() -> Option<Arc<Value>> {
    GLOBAL_CONFIG.get().cloned()
}
