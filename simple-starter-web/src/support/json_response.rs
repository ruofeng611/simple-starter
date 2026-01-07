//! 统一 JSON 响应体处理模块。
//!
//! 本模块定义了标准的 Web API 响应格式 `JsonResponse`，
//! 并提供了 `json_response_wrap!` 声明宏，用于在 handler 函数内部
//! 方便地构建此格式的响应，同时自动处理 `Result` 类型的错误。

use crate::SimpleAppWebError;
use serde::Serialize;
use serde_json::Value;
use simple_starter_core::AppCoreUtil;
use simple_starter_core::tracing::warn;

/// 标准化的 Web API 响应体结构。
///
/// 该结构遵循常见的 REST ful API 设计规范，包含状态码、消息、服务/功能元信息以及实际数据。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonResponse {
    /// 业务状态码 (例如: 200 成功, 400 参数错误, 500 服务器错误)。
    pub code: i32,
    /// 对 `code` 的人类可读描述。
    pub message: String,
    /// (可选) 从全局配置 `app.name` 中获取的服务名称，用于分布式追踪。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    /// (可选) 调用此响应的功能名，便于调试和日志关联。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    /// (可选) 实际的业务数据。如果序列化结果为 `null`，则不会出现在最终 JSON 中。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// `json_response_wrap!` 宏的入口。
///
/// 此宏提供了一种声明式的方式来包装 handler 的核心逻辑，使其返回 `JsonResponse`。
/// 它通过匹配不同的参数模式，最终都调用 `json_response_wrap_impl!` 宏来执行实际逻辑。
///
/// # 使用说明
/// 在 handler 函数中，将您的核心业务逻辑（必须返回 `Result<T, SimpleAppWebError>`）
/// 放在一个代码块 `{}` 中，并用此宏包裹。您可以根据需要传入 `code`、`message` 或 `function_name`。
///
/// ```ignore
/// let resp = json_response_wrap!(
///     code = 201,
///     message = "Created",
///     function_name = "create_user",
///     {
///         let user = create_user_in_db(...).await?;
///         Ok(user)
///     }
/// );
/// ```
#[macro_export]
macro_rules! json_response_wrap {
    // 全参数模式: code, message, function_name
    (code=$code:expr, message=$msg:expr, function_name=$func_name:expr, $block:block) => {{
        $crate::json_response_wrap_impl!($code, $msg, Some($func_name), $block)
    }};
    // 两参数模式: code, message
    (code=$code:expr, message=$msg:expr, $block:block) => {{
        $crate::json_response_wrap_impl!($code, $msg, None, $block)
    }};
    // 两参数模式: code, function_name
    (code=$code:expr, function_name=$func_name:expr, $block:block) => {{
        $crate::json_response_wrap_impl!($code, "操作成功", Some($func_name), $block)
    }};
    // 两参数模式: message, function_name
    (message=$msg:expr, function_name=$func_name:expr, $block:block) => {{
        $crate::json_response_wrap_impl!(200, $msg, Some($func_name), $block)
    }};
    // 单参数模式: code
    (code=$code:expr, $block:block) => {{
        $crate::json_response_wrap_impl!($code, "操作成功", None, $block)
    }};
    // 单参数模式: message
    (message=$msg:expr, $block:block) => {{
        $crate::json_response_wrap_impl!(200, $msg, None, $block)
    }};
    // 单参数模式: function_name
    (function_name=$func_name:expr, $block:block) => {{
        $crate::json_response_wrap_impl!(200, "操作成功", Some($func_name), $block)
    }};
    // 无参数模式
    ($block:block) => {{
        $crate::json_response_wrap_impl!(200, "操作成功", None, $block)
    }};
}

/// `json_response_wrap!` 宏的实际实现。
///
/// 此宏负责执行异步代码块，并调用 `process_data` 函数来构造最终的 `JsonResponse`。
///
/// # 参数
/// - `$code`: 期望的成功状态码。
/// - `$msg`: 期望的成功消息。
/// - `$func_name`: 可选的功能名。
/// - `$block`: 包含核心业务逻辑的代码块，必须返回 `Result<_, SimpleAppWebError>`。
#[macro_export]
macro_rules! json_response_wrap_impl {
    ($code:expr, $msg:expr, $func_name:expr, $block:block) => {{
        use $crate::SimpleAppWebError;
        use $crate::process_data;
        // 执行用户提供的异步代码块，得到一个 Result
        let result: Result<_, SimpleAppWebError> = async move $block.await;
        // 调用核心处理函数
        process_data($code, $msg, $func_name, result)
    }};
}

/// 核心处理函数，将 `Result` 转换为 `JsonResponse`。
///
/// # 流程说明
/// 1. **成功路径 (`Ok(data)`)**:
///    - 使用传入的 `$code` 和 `$msg`。
///    - 尝试将 `data` 序列化为 `serde_json::Value`。
///    - 如果序列化结果是 `Value::Null`，则 `data` 字段设为 `None`，避免在 JSON 中出现 `"data": null`。
/// 2. **失败路径 (`Err(err)`)**:
///    - 直接使用 `err` 自身携带的 `code` 和 `message`。
///    - 从 `err` 中提取附加的错误数据（如果有）。
/// 3. **通用字段**:
///    - `service_name` 和 `function_name` 会被填充到响应中。
pub fn process_data<T>(
    code: i32,
    message: &str,
    function_name: Option<&str>,
    result: Result<T, SimpleAppWebError>,
) -> JsonResponse
where
    T: Serialize,
{
    // 从全局配置中尝试获取服务名
    let service_name = match AppCoreUtil::get_config_value_by_path("app.name") {
        Ok(v) => v.as_str().map(|s| s.to_string()),
        Err(_) => None,
    };
    match result {
        Ok(data) => JsonResponse {
            code,
            message: message.to_string(),
            service_name,
            function_name: function_name.map(|s| s.to_string()),
            data: match serde_json::to_value(data) {
                Ok(Value::Null) => None,
                Ok(v) => Some(v),
                Err(e) => {
                    warn!("Serialize response data failed: {}", e);
                    None
                }
            },
        },
        Err(err) => JsonResponse {
            code: err.code(),
            message: err.message(),
            service_name,
            function_name: function_name.map(|s| s.to_string()),
            data: err.into_error_data(),
        },
    }
}