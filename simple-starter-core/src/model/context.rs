//! # 应用上下文（插件与钩子的协作面，对标 Spring ApplicationContext）
//!
//! `AppContext` 以**所有权**聚合全部运行期协作资源，由 `Application` 持有
//! （字段 `context`），插件就绪/收尾期（components_ready / finalize）与启动/关闭
//! 钩子均以 `&mut AppContext` 访问（装配期 assemble 仅经 [`Extensions`] 协作面）：
//!
//! - `config`：合并后的全局配置（启动时写入一次，`Arc` 支持廉价 clone）
//! - `container`：组件容器（组件查询与按 trait/类型解析）
//! - `extensions`：扩展存储（插件间挂载/消费自定义协作数据）
//! - `task_spawns`：异步任务创建工厂（插件/钩子注册后台任务）
//!
//! 分类原则：**运行期协作资源进 AppContext，构建期资源（插件注册、运行时工厂、
//! 默认配置、启动/关闭钩子、主循环钩子）留在 `Application` 上**——插件与钩子
//! 只能看到并修改协作资源，无法触碰框架自身的构建期配置面。
//!
//! 全局快照访问点见 [`global_context`]（无上下文传播场景的逃生舱）。

pub(crate) mod global_context;

use crate::model::component::ComponentContainer;
use crate::model::extensions::Extensions;
use crate::utils::core_util::{get_config_to_struct, get_config_value_by_path, TomlConfigError};
use crate::BoxFuture;
use crate::LogExpectExt;
use serde::Deserialize;
use std::any::Any;
use std::future::Future;
use std::sync::{Arc, OnceLock};
use tokio_util::sync::CancellationToken;
use toml::Value;

/// 异步任务工厂函数签名
///
/// 接收取消令牌（CancellationToken），返回一个可执行的 Future。
/// 用于将后台任务注入到运行时中。
pub(crate) type TaskSpawnsFactory = Box<dyn FnOnce(CancellationToken) -> BoxFuture<anyhow::Result<()>> + Send>;

/// 应用上下文
///
/// 由 `Application` 持有，插件就绪/收尾期（components_ready / finalize）
/// 与启动/关闭钩子以 `&mut AppContext` 访问（装配期仅经 `Extensions`）。
pub struct AppContext {
    /// 合并后的全局配置（启动时写入一次；Arc 支持廉价 clone 传入后台任务）
    config: OnceLock<Arc<Value>>,
    /// 组件容器
    container: Arc<ComponentContainer>,
    /// 扩展存储（插件协作数据）
    extensions: Extensions,
    /// 异步任务创建工厂列表（插件/钩子注册后台任务）
    task_spawns: Vec<TaskSpawnsFactory>,
}

impl AppContext {
    /// 创建空的应用上下文（由 `Application::new` 调用）
    pub(crate) fn new() -> Self {
        Self {
            config: OnceLock::new(),
            container: Arc::new(ComponentContainer::new()),
            extensions: Extensions::new(),
            task_spawns: Vec::new(),
        }
    }

    /// 写入合并后的全局配置（只能写入一次，启动时由 `Application` 调用）
    pub(crate) fn set_config(&self, value: Arc<Value>) -> Result<(), Arc<Value>> {
        self.config.set(value)
    }

    /// 获取合并后的全局配置引用（Arc，可廉价 clone 传入后台任务）
    ///
    /// 仅在 `run()` 配置加载完成后可用；在此之前调用 panic（fail-fast）。
    pub fn config(&self) -> &Arc<Value> {
        self.config
            .get()
            .log_expect("Global configuration not initialized. Ensure Application::run() is called.")
    }

    /// 通过点分隔路径获取配置值 (例如 "web.port")
    pub fn get_config_value_by_path(&self, path: &str) -> Option<&Value> {
        get_config_value_by_path(self.config(), path)
    }

    /// 获取配置并反序列化为结构体
    pub fn get_config_to_struct<T>(&self, path: &str) -> Result<T, TomlConfigError>
    where
        T: for<'de> Deserialize<'de>,
    {
        get_config_to_struct(self.config(), path)
    }

    /// 获取组件容器（组件查询与依赖解析）
    pub fn container(&self) -> &Arc<ComponentContainer> {
        &self.container
    }

    /// 插入一个扩展值。
    ///
    /// 如果同类型已存在，返回旧值。
    pub fn insert_extension<T: Any + Send>(&mut self, val: T) -> Option<T> {
        self.extensions.insert(val)
    }

    /// 获取不可变的扩展引用。
    pub fn get_extension<T: Any + Send>(&self) -> Option<&T> {
        self.extensions.get::<T>()
    }

    /// 获取可变的扩展引用。
    pub fn get_extension_mut<T: Any + Send>(&mut self) -> Option<&mut T> {
        self.extensions.get_mut::<T>()
    }

    /// 移除并返回指定类型的扩展值。
    pub fn remove_extension<T: Any + Send>(&mut self) -> Option<T> {
        self.extensions.remove::<T>()
    }

    /// 检查是否包含指定类型的扩展。
    pub fn contains_extension<T: Any + Send>(&self) -> bool {
        self.extensions.contains::<T>()
    }

    /// 在应用启动时的上下文中添加异步任务创建工厂(供插件与钩子使用)
    pub fn add_task_spawn_factory_in_context<F, Fut>(&mut self, f: F)
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.task_spawns
            .push(Box::new(move |token| Box::pin(f(token))));
    }

    /// 取出全部异步任务创建工厂（启动阶段收尾时由 `Application` 消费）
    pub(crate) fn take_task_spawns(&mut self) -> Vec<TaskSpawnsFactory> {
        std::mem::take(&mut self.task_spawns)
    }

    /// 可变借用扩展存储（仅启动流程装配期使用：插件 `assemble` 阶段只被授予扩展面，
    /// 组件容器、全局配置与任务工厂均不可见）
    pub(crate) fn extensions_mut(&mut self) -> &mut Extensions {
        &mut self.extensions
    }
}
