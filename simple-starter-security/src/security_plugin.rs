//! Security 插件。

use std::sync::Arc;

use async_trait::async_trait;
use simple_starter_core::{anyhow, Application, Plugin};
use simple_starter_web::{axum, WebExtensionRegistry};

use crate::auth_middleware::{
    security_middleware, DefaultPermissionChecker, DefaultSecurityErrorHandler, PermissionChecker,
    SecurityErrorHandler, SecurityMiddlewareState, UserInfoProvider,
};
use crate::resource::ResourceEntry;
use crate::whitelist::Whitelist;

/// Security 插件。
///
/// 提供编译期资源收集、运行时白名单放行、用户认证、权限校验能力。
pub struct SecurityPlugin {
    whitelist: Whitelist,
    user_info_provider: Option<Arc<dyn UserInfoProvider>>,
    permission_checker: Arc<dyn PermissionChecker>,
    error_handler: Arc<dyn SecurityErrorHandler>,
}

impl SecurityPlugin {
    /// 创建 SecurityPlugin 实例。
    pub fn new() -> Self {
        Self {
            whitelist: Whitelist::new(),
            user_info_provider: None,
            permission_checker: Arc::new(DefaultPermissionChecker),
            error_handler: Arc::new(DefaultSecurityErrorHandler),
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
    pub fn with_user_info_provider(mut self, provider: Arc<dyn UserInfoProvider>) -> Self {
        self.user_info_provider = Some(provider);
        self
    }

    /// 设置权限检查器。
    ///
    /// 默认使用 [`DefaultPermissionChecker`]，直接校验用户上下文的 `resource_ids`。
    pub fn with_permission_checker(mut self, checker: Arc<dyn PermissionChecker>) -> Self {
        self.permission_checker = checker;
        self
    }

    /// 设置自定义错误处理器。
    ///
    /// 默认使用 [`DefaultSecurityErrorHandler`]，返回标准 HTTP 401/403。
    pub fn with_error_handler(mut self, handler: Arc<dyn SecurityErrorHandler>) -> Self {
        self.error_handler = handler;
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

    /// 初始化阶段。
    ///
    /// 向 WebPlugin 的 `WebExtensionRegistry` 注册安全中间件。
    async fn init(&mut self, ctx: &mut Application) -> anyhow::Result<()> {
        let registry = ctx
            .get_extension_mut::<WebExtensionRegistry>()
            .ok_or_else(|| anyhow::anyhow!("WebExtensionRegistry not found. Did WebPlugin init?"))?;

        // 构建资源映射表：path_pattern -> resource_id
        let mut resource_map = std::collections::HashMap::new();
        for entry in inventory::iter::<ResourceEntry> {
            // 运行时校验路径唯一性
            if let Some(existing) = resource_map.get(entry.path_pattern) {
                return Err(anyhow::anyhow!(
                    "Duplicate path pattern '{}' detected: resource_id '{}' conflicts with '{}'",
                    entry.path_pattern,
                    entry.resource_id,
                    existing
                ));
            }
            resource_map.insert(entry.path_pattern.to_string(), entry.resource_id.to_string());
        }

        let state = SecurityMiddlewareState {
            whitelist: self.whitelist.clone(),
            user_info_provider: self.user_info_provider.clone(),
            permission_checker: Arc::clone(&self.permission_checker),
            resource_map: Arc::new(resource_map),
            error_handler: self.error_handler.clone(),
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
