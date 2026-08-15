//! Security 插件。

use std::sync::Arc;

use async_trait::async_trait;
use simple_starter_core::{AppCoreUtil, Application, Injectable, Plugin, anyhow};
use simple_starter_web::{WebExtensionRegistry, axum};
use toml::Value;

use crate::auth_middleware::{
    PermissionChecker, SecurityErrorHandler, SecurityMiddlewareState, UserInfoProvider,
    security_middleware,
};
use crate::resource::ResourceEntry;
use crate::whitelist::Whitelist;

/// 基础路径提供者。
///
/// 用于在构建资源映射表时，为路径模式添加 Web 服务的基础路径前缀。
pub trait BasePathProvider: Injectable {
    fn base_path(&self) -> String;
}

/// 默认基础路径提供者。
///
/// 从 TOML 配置的 `web.base_path` 读取基础路径，若未配置则返回空字符串。
///
/// 以条件注册方式参与组件装配：当用户未提供任何 [`BasePathProvider`] 实现时注册本默认实现，
/// 否则自动退位让位给用户实现。
#[simple_starter_macro::component(condition = simple_starter_core::ComponentCondition::on_missing_trait::<dyn BasePathProvider>())]
pub struct DefaultBasePathProvider;

#[simple_starter_macro::injectable]
impl BasePathProvider for DefaultBasePathProvider {
    fn base_path(&self) -> String {
        AppCoreUtil::get_config_value_by_path("web.base_path")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }
}

/// Security 插件。
///
/// 提供编译期资源收集、运行时白名单放行、用户认证、权限校验能力。
///
/// # 使用方式
///
/// ```ignore
/// simple_starter_core::Application::new()
///     .register_plugin(WebPlugin::new())
///     .register_plugin(SecurityPlugin::new()
///         .add_whitelist(Some("GET"), "/health"))
///     .run();
/// ```
///
/// 四个协作接口均通过组件仓库获取：
/// - [`PermissionChecker`]、[`SecurityErrorHandler`]、[`BasePathProvider`] 提供默认实现
///   （条件注册，用户实现存在时自动退位）；
/// - [`UserInfoProvider`] 无默认实现，未提供时拒绝所有请求。
///
/// 用户通过 `#[component]` + `#[injectable]` 注册自定义实现即可覆盖默认行为。
pub struct SecurityPlugin {
    whitelist: Whitelist,
}

impl SecurityPlugin {
    /// 创建 SecurityPlugin 实例。
    pub fn new() -> Self {
        Self {
            whitelist: Whitelist::new(),
        }
    }

    /// 添加白名单条目。
    ///
    /// # 参数
    /// - `method`: HTTP 方法，如 `"GET"`、`"POST"`；`None` 表示所有方法
    /// - `path_pattern`: 路径模式，如 `"/login"` 或 `"/public/*"`
    pub fn add_whitelist(mut self, method: Option<&str>, path_pattern: impl Into<String>) -> Self {
        self.whitelist.add(method, path_pattern);
        self
    }

    /// 收集所有编译期注册的安全资源。
    ///
    /// 由于资源通过 `inventory` 在编译期静态收集，可以在任何时刻调用此方法获取。
    pub fn collect_resources() -> Vec<&'static ResourceEntry> {
        inventory::iter::<ResourceEntry>().collect()
    }
}

#[async_trait]
impl Plugin for SecurityPlugin {
    fn name(&self) -> &'static str {
        "SecurityPlugin"
    }

    fn dependencies(&self) -> &[&'static str] {
        &["WebPlugin"]
    }

    fn default_config(&self) -> Value {
        let table = toml::toml! {
            [security]
            log_warn = false
        };
        Value::Table(table)
    }

    /// 装配期。
    ///
    /// 校验 WebPlugin 已将 `WebExtensionRegistry` 装配进应用上下文（fail-fast 早于组件加载）。
    async fn assemble(&mut self, ctx: &mut Application) -> anyhow::Result<()> {
        ctx.get_extension::<WebExtensionRegistry>().ok_or_else(|| {
            anyhow::anyhow!("WebExtensionRegistry not found. Did WebPlugin assemble?")
        })?;
        Ok(())
    }

    /// 组件就绪期。
    ///
    /// 从组件仓库获取四个协作接口实例（默认实现经条件注册保证存在、用户实现存在时自动退位），
    /// 构建资源映射表与安全中间件状态，并注册到 WebPlugin 的 `WebExtensionRegistry`。
    async fn components_ready(&mut self, ctx: &mut Application) -> anyhow::Result<()> {
        // 读取是否打印警告日志的配置
        let log_warn = AppCoreUtil::get_config_value_by_path("security.log_warn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let registry = ctx.get_extension_mut::<WebExtensionRegistry>().ok_or_else(|| {
            anyhow::anyhow!("WebExtensionRegistry not found. Did WebPlugin assemble?")
        })?;

        // 获取组件：默认实现按条件注册保证存在；UserInfoProvider 无默认实现，获取失败降级为 None（拒绝所有请求）
        let base_path_provider =
            AppCoreUtil::get_component_by_trait::<dyn BasePathProvider>()?;
        let permission_checker = AppCoreUtil::get_component_by_trait::<dyn PermissionChecker>()?;
        let error_handler = AppCoreUtil::get_component_by_trait::<dyn SecurityErrorHandler>()?;
        let user_info_provider =
            AppCoreUtil::get_component_by_trait::<dyn UserInfoProvider>().ok();

        // 构建资源映射表：path_pattern -> resource_id
        let base_path = base_path_provider.base_path();
        let mut resource_map = std::collections::HashMap::new();
        for entry in inventory::iter::<ResourceEntry> {
            let full_pattern = if base_path.is_empty() {
                entry.path_pattern.to_string()
            } else {
                let base = base_path.trim_end_matches('/');
                format!("{}{}", base, entry.path_pattern)
            };

            // 运行时校验路径唯一性
            if let Some(existing) = resource_map.get(&full_pattern) {
                return Err(anyhow::anyhow!(
                    "Duplicate path pattern '{}' detected: resource_id '{}' conflicts with '{}'",
                    full_pattern,
                    entry.resource_id,
                    existing
                ));
            }
            resource_map.insert(full_pattern, entry.resource_id.to_string());
        }

        let state = SecurityMiddlewareState {
            whitelist: self.whitelist.clone(),
            user_info_provider,
            permission_checker,
            resource_map: Arc::new(resource_map),
            error_handler,
            log_warn,
        };

        registry.add_middleware(move |router| {
            router.layer(axum::middleware::from_fn_with_state(
                state.clone(),
                security_middleware,
            ))
        });

        Ok(())
    }
}
