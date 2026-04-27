use crate::application::Application;
use async_trait::async_trait;
use toml::Value;

/// 插件 Trait
///
/// 允许扩展系统功能，插件会在组件加载完毕后初始化。
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

    /// 初始化插件
    ///
    /// 在此处可以访问 `Application` 上下文，添加任务。
    async fn init(&mut self, ctx: &mut Application) -> anyhow::Result<()>;

    /// 后置初始化钩子（可选）
    ///
    /// 在所有插件的 `init` 都执行完毕后，按拓扑顺序调用。
    /// 适合需要在其他插件初始化完成后再进行的操作，例如消费由其他插件注册的扩展。
    async fn post_init(&mut self, _ctx: &mut Application) -> anyhow::Result<()> {
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
