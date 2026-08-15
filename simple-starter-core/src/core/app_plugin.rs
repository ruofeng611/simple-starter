use crate::application::Application;
use async_trait::async_trait;
use toml::Value;

/// 插件 Trait
///
/// 允许扩展系统功能。插件生命周期按职责划分为三个周期（均按插件拓扑顺序执行）：
///
/// 1. `assemble`（装配期）：组件加载前，用于装配扩展注册表。
/// 2. `components_ready`（组件就绪期）：组件全部创建并初始化完成后，用于获取组件实例、
///    注入插件协作结构、执行组件启动后配置。
/// 3. `finalize`（收尾期）：所有插件协作完毕后，用于消费扩展注册表、构建并启动服务。
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
    /// 组件加载前的唯一窗口。在此处将扩展注册表放入
    /// `Application` 上下文，供其他插件继续填充。
    async fn assemble(&mut self, ctx: &mut Application) -> anyhow::Result<()>;

    /// 组件就绪钩子（可选）
    ///
    /// 在组件仓库加载（create + init）完成后、`finalize` 之前按拓扑顺序调用。
    /// 此时组件已定型，适合获取组件实例（含条件注册的默认实现与用户覆盖）、
    /// 构建依赖组件的插件协作结构（如中间件状态）。
    async fn components_ready(&mut self, _ctx: &mut Application) -> anyhow::Result<()> {
        Ok(())
    }

    /// 收尾钩子（可选）
    ///
    /// 在所有插件的 `assemble` 与 `components_ready` 都执行完毕后，按拓扑顺序调用。
    /// 适合消费由其他插件填充完毕的扩展注册表、构建并启动服务。
    async fn finalize(&mut self, _ctx: &mut Application) -> anyhow::Result<()> {
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
