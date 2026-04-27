//! Security 插件。

use std::sync::Arc;

use async_trait::async_trait;
use simple_starter_core::{AppCoreUtil, Application, Plugin, anyhow};
use simple_starter_web::{WebExtensionRegistry, axum};
use toml::Value;

use crate::auth_middleware::{
    DefaultPermissionChecker, DefaultSecurityErrorHandler, PermissionChecker, SecurityErrorHandler,
    SecurityMiddlewareState, UserInfoProvider, security_middleware,
};
use crate::resource::ResourceEntry;
use crate::whitelist::Whitelist;

/// 基础路径提供者。
///
/// 用于在构建资源映射表时，为路径模式添加 Web 服务的基础路径前缀。
pub trait BasePathProvider: Send + Sync {
    fn base_path(&self) -> String;
}

/// 默认基础路径提供者。
///
/// 从 TOML 配置的 `web.base_path` 读取基础路径，若未配置则返回空字符串。
pub struct DefaultBasePathProvider;

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
pub struct SecurityPlugin {
    whitelist: Whitelist,
    user_info_provider: Option<Arc<dyn UserInfoProvider>>,
    permission_checker: Arc<dyn PermissionChecker>,
    error_handler: Arc<dyn SecurityErrorHandler>,
    base_path_provider: Arc<dyn BasePathProvider>,
}

impl SecurityPlugin {
    /// 创建 SecurityPlugin 实例。
    pub fn new() -> Self {
        Self {
            whitelist: Whitelist::new(),
            user_info_provider: None,
            permission_checker: Arc::new(DefaultPermissionChecker),
            error_handler: Arc::new(DefaultSecurityErrorHandler),
            base_path_provider: Arc::new(DefaultBasePathProvider),
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

    /// 设置用户信息提供者。
    pub fn with_user_info_provider<T: UserInfoProvider + 'static>(mut self, provider: T) -> Self {
        self.user_info_provider = Some(Arc::new(provider));
        self
    }

    /// 设置权限检查器。
    ///
    /// 默认使用 [`DefaultPermissionChecker`]，直接校验用户上下文的 `resource_ids` 集合。
    pub fn with_permission_checker<T: PermissionChecker + 'static>(mut self, checker: T) -> Self {
        self.permission_checker = Arc::new(checker);
        self
    }

    /// 设置自定义错误处理器。
    ///
    /// 默认使用 [`DefaultSecurityErrorHandler`]，返回标准 HTTP 401/403。
    pub fn with_error_handler<T: SecurityErrorHandler + 'static>(mut self, handler: T) -> Self {
        self.error_handler = Arc::new(handler);
        self
    }

    /// 设置基础路径提供者。
    ///
    /// 默认使用 [`DefaultBasePathProvider`]，从 TOML 配置 `web.base_path` 读取。
    pub fn with_base_path_provider<T: BasePathProvider + 'static>(mut self, provider: T) -> Self {
        self.base_path_provider = Arc::new(provider);
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

    /// 初始化阶段。
    ///
    /// 向 WebPlugin 的 `WebExtensionRegistry` 注册安全中间件。
    async fn init(&mut self, ctx: &mut Application) -> anyhow::Result<()> {
        // 读取是否打印警告日志的配置
        let log_warn = AppCoreUtil::get_config_value_by_path("security.log_warn")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let registry = ctx
            .get_extension_mut::<WebExtensionRegistry>()
            .ok_or_else(|| {
                anyhow::anyhow!("WebExtensionRegistry not found. Did WebPlugin init?")
            })?;

        // 构建资源映射表：path_pattern -> resource_id
        let base_path = self.base_path_provider.base_path();
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
            user_info_provider: self.user_info_provider.clone(),
            permission_checker: self.permission_checker.clone(),
            resource_map: Arc::new(resource_map),
            error_handler: self.error_handler.clone(),
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
