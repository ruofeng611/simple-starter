//! # 插件 Trait
//!
//! 允许扩展系统功能。插件生命周期按职责划分为三个周期（均按插件拓扑顺序执行）：
//!
//! 1. `assemble`（装配期）：组件加载前，用于装配扩展注册表。此阶段插件**仅被授予
//!    扩展存储**（`Extensions`）——组件尚未创建、后台任务工厂注册窗口未开启，
//!    组件容器、全局配置与任务工厂均不可见。
//! 2. `components_ready`（组件就绪期）：组件全部创建并初始化完成后，用于获取组件实例、
//!    注入插件协作结构、执行组件启动后配置。
//! 3. `finalize`（收尾期）：所有插件协作完毕后，用于消费扩展注册表、构建并启动服务。
//!
//! 装配期经 [`Extensions`] 协作（只授予扩展面），就绪期与收尾期经 [`AppContext`]
//! 协作面访问运行期资源（组件容器、全局配置、扩展存储、任务工厂注册器），
//! 构建期资源（插件注册、运行时工厂、钩子等）仅在 `Application` 上可用，
//! 插件不可触碰。

use crate::model::context::AppContext;
use crate::model::extensions::Extensions;
use async_trait::async_trait;
use toml::Value;

/// 插件 Trait
#[async_trait]
pub trait Plugin: Send {
    /// 插件唯一名称
    fn name(&self) -> &'static str;

    /// 声明依赖的其他插件名称（用于拓扑排序，确保初始化顺序）
    fn dependencies(&self) -> &[&'static str] {
        &[]
    }

    /// 提供插件的默认配置（将被合并到全局配置中）
    fn default_config(&self) -> Value {
        Value::Table(toml::value::Table::new())
    }

    /// 装配插件
    ///
    /// 组件加载前的唯一窗口。此阶段插件仅被授予扩展存储：在此处将本插件的
    /// 扩展注册表放入 [`Extensions`]，供其他插件继续填充（如依赖插件的注册表校验）。
    /// 组件容器（尚未创建组件）、全局配置与任务工厂注册在本阶段不可见——
    /// 需要组件实例或注册后台任务时，应推迟到 `components_ready` / `finalize`。
    async fn assemble(&mut self, _extensions: &mut Extensions) -> anyhow::Result<()> {
        Ok(())
    }

    /// 组件就绪钩子（可选）
    ///
    /// 在组件仓库加载（create + init）完成后、`finalize` 之前按拓扑顺序调用。
    /// 此时组件已定型，适合获取组件实例（含条件注册的默认实现与用户覆盖）、
    /// 构建依赖组件的插件协作结构（如中间件状态）。
    async fn components_ready(&mut self, _ctx: &mut AppContext) -> anyhow::Result<()> {
        Ok(())
    }

    /// 收尾钩子（可选）
    ///
    /// 在所有插件的 `assemble` 与 `components_ready` 都执行完毕后，按拓扑顺序调用。
    /// 适合消费由其他插件填充完毕的扩展注册表、构建并启动服务。
    async fn finalize(&mut self, _ctx: &mut AppContext) -> anyhow::Result<()> {
        Ok(())
    }

    /// 可选的关闭钩子
    ///
    /// 应用退出时，按照初始化相反的顺序调用。
    async fn shutdown_hook(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    /// 是否打印标准的生命周期日志
    fn should_log(&self) -> bool {
        true
    }
}
