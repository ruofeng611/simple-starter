//! 认证与授权中间件。

use std::sync::Arc;

use simple_starter_web::axum::{
    body::Body,
    extract::{MatchedPath, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use simple_starter_core::tracing;
use crate::whitelist::Whitelist;

/// 用户上下文。
///
/// 由 `UserInfoProvider` 从请求中解析，附加到 Request extensions 中供后续使用。
#[derive(Debug, Clone)]
pub struct UserContext {
    /// 用户唯一标识
    pub user_id: String,
    /// 当前用户拥有的资源标识列表
    pub resource_ids: Vec<String>,
    /// 用户是否被禁用
    pub is_disabled: bool,
    /// 用户过期时间
    pub expired_at: Option<std::time::SystemTime>,
    /// 扩展字段，供业务自定义
    pub extra: Option<serde_json::Value>,
}

impl UserContext {
    pub fn has_resource(&self, resource_id: &str) -> bool {
        self.resource_ids.iter().any(|r| r == resource_id)
    }

    /// 检查用户是否已过期。
    ///
    /// 当 `expired_at` 为 `None`（未设置过期时间）时，返回 `false`，表示永不过期。
    pub fn is_expired(&self) -> bool {
        self.expired_at.map_or(false, |t| t < std::time::SystemTime::now())
    }

    /// 检查用户是否处于有效状态（未禁用且未过期）。
    pub fn is_active(&self) -> bool {
        !self.is_disabled && !self.is_expired()
    }
}

/// 用户信息提供者。
///
/// 负责从 HTTP 请求中解析出当前用户上下文（如从 JWT Token、Session Cookie 等）。
#[async_trait::async_trait]
pub trait UserInfoProvider: Send + Sync {
    async fn get_user_context(&self, req: &Request<Body>) -> Option<UserContext>;
}

/// 权限检查器。
///
/// 由使用者实现，根据用户上下文和当前资源标识决定是否放行。
///
/// 默认实现 [`DefaultPermissionChecker`] 直接检查用户上下文的 `resource_ids` 列表。
#[async_trait::async_trait]
pub trait PermissionChecker: Send + Sync {
    /// 检查用户是否有权访问指定资源。
    async fn check(&self, user_ctx: &UserContext, resource_id: &str) -> bool;
}

/// 默认权限检查器。
///
/// 直接调用 [`UserContext::has_resource`] 进行判断。
pub struct DefaultPermissionChecker;

#[async_trait::async_trait]
impl PermissionChecker for DefaultPermissionChecker {
    async fn check(&self, user_ctx: &UserContext, resource_id: &str) -> bool {
        user_ctx.has_resource(resource_id)
    }
}

/// 安全错误类型。
///
/// 枚举中间件中所有可能发生的授权/认证错误，供 [`SecurityErrorHandler`] 精确处理。
#[derive(Debug, Clone, thiserror::Error)]
pub enum SecurityError {
    /// 用户被禁用。
    #[error("User '{user_id}' is disabled")]
    UserDisabled { user_id: String },
    /// 用户会话已过期。
    #[error("User '{user_id}' has expired")]
    UserExpired { user_id: String },
    /// 无法获取当前请求的 MatchedPath。
    #[error("MatchedPath not available")]
    MatchedPathUnavailable,
    /// 请求路径未注册对应资源。
    #[error("No resource registered for path pattern '{pattern}'")]
    ResourceNotFound { pattern: String },
    /// 权限校验不通过。
    #[error("User '{user_id}' denied access to resource '{resource_id}'")]
    PermissionDenied { user_id: String, resource_id: String },
}

/// 安全错误处理器。
///
/// 允许使用者自定义认证/授权失败时的 HTTP 响应，
/// 例如返回带有业务状态码的 JSON 结构体，而非固定的 401/403 状态码。
#[async_trait::async_trait]
pub trait SecurityErrorHandler: Send + Sync {
    /// 未认证（无有效用户上下文）时的响应。
    async fn unauthorized(&self, req: &Request<Body>) -> Response;

    /// 无权限（资源不存在、未配置检查器、或检查不通过）时的响应。
    ///
    /// `error` 参数提供精确的错误类型与诊断信息，可用于日志或自定义错误消息。
    async fn forbidden(&self, req: &Request<Body>, error: &SecurityError) -> Response;
}

/// 默认错误处理器。
///
/// 直接返回标准的 HTTP 401/403 状态码。
pub struct DefaultSecurityErrorHandler;

#[async_trait::async_trait]
impl SecurityErrorHandler for DefaultSecurityErrorHandler {
    async fn unauthorized(&self, _req: &Request<Body>) -> Response {
        StatusCode::UNAUTHORIZED.into_response()
    }

    async fn forbidden(&self, _req: &Request<Body>, _error: &SecurityError) -> Response {
        StatusCode::FORBIDDEN.into_response()
    }
}

/// 中间件共享状态。
#[derive(Clone)]
pub struct SecurityMiddlewareState {
    pub whitelist: Whitelist,
    pub user_info_provider: Option<Arc<dyn UserInfoProvider>>,
    pub permission_checker: Arc<dyn PermissionChecker>,
    pub resource_map: Arc<std::collections::HashMap<String, String>>, // path_pattern -> resource_id
    pub error_handler: Arc<dyn SecurityErrorHandler>,
}

/// Security 全局中间件。
///
/// 执行流程：
/// 1. 白名单检查 → 放行
/// 2. 解析用户上下文 → 无则 401
/// 3. 用户状态检查（禁用/过期） → 无效则 403
/// 4. 权限检查 → 无则 403
pub async fn security_middleware(
    State(state): State<SecurityMiddlewareState>,
    matched_path: Option<MatchedPath>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let method_str = req.method().as_str().to_string();
    let path_str = req.uri().path().to_string();

    // === 阶段 1: 白名单检查 ===
    if state.whitelist.is_allowed(&method_str, &path_str) {
        return next.run(req).await;
    }

    // === 阶段 2: 解析用户上下文 ===
    let user_ctx = match state.user_info_provider {
        Some(ref provider) => match provider.get_user_context(&req).await {
            Some(ctx) => ctx,
            None => {
                return state.error_handler.unauthorized(&req).await;
            }
        },
        None => {
            tracing::warn!("SecurityPlugin: UserInfoProvider not configured, rejecting request");
            return state.error_handler.unauthorized(&req).await;
        }
    };

    // === 阶段 3: 用户状态检查 ===
    if user_ctx.is_disabled {
        tracing::warn!("SecurityPlugin: User '{}' is disabled", user_ctx.user_id);
        return state
            .error_handler
            .forbidden(
                &req,
                &SecurityError::UserDisabled {
                    user_id: user_ctx.user_id.clone(),
                },
            )
            .await;
    }
    if user_ctx.is_expired() {
        tracing::warn!("SecurityPlugin: User '{}' has expired", user_ctx.user_id);
        return state
            .error_handler
            .forbidden(
                &req,
                &SecurityError::UserExpired {
                    user_id: user_ctx.user_id.clone(),
                },
            )
            .await;
    }

    // 将用户上下文附加到请求，供下游 handler 使用
    req.extensions_mut().insert(user_ctx.clone());

    // === 阶段 4: 权限检查 ===
    // 获取当前请求匹配的路径模式
    let pattern = match matched_path {
        Some(mp) => mp.as_str().to_string(),
        None => {
            tracing::warn!(
                "SecurityPlugin: MatchedPath not available for {} {}",
                method_str, path_str
            );
            return state
                .error_handler
                .forbidden(&req, &SecurityError::MatchedPathUnavailable)
                .await;
        }
    };

    // 查找路径模式对应的资源标识
    let resource_id = match state.resource_map.get(&pattern) {
        Some(id) => id.clone(),
        None => {
            tracing::warn!(
                "SecurityPlugin: No resource registered for path pattern '{}'",
                pattern
            );
            return state
                .error_handler
                .forbidden(
                    &req,
                    &SecurityError::ResourceNotFound {
                        pattern: pattern.clone(),
                    },
                )
                .await;
        }
    };

    // 调用权限检查逻辑
    let allowed = state.permission_checker.check(&user_ctx, &resource_id).await;

    if allowed {
        next.run(req).await
    } else {
        tracing::warn!(
            "SecurityPlugin: User '{}' denied access to resource '{}'",
            user_ctx.user_id, resource_id
        );
        state
            .error_handler
            .forbidden(
                &req,
                &SecurityError::PermissionDenied {
                    user_id: user_ctx.user_id.clone(),
                    resource_id,
                },
            )
            .await
    }
}
