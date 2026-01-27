//! 统一 JSON 响应体处理模块。
//!
//! 定义了标准的 `JsonResponse` 结构，并提供了宏来简化 Controller 的编写。

use crate::SimpleAppWebError;
use serde::Serialize;
use serde_json::Value;
use simple_starter_core::AppCoreUtil;
use simple_starter_core::tracing::{error, warn};

/// 标准化 Web API 响应结构
///
/// 遵循 { code, message, data } 的常见格式。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonResponse {
    /// 业务状态码
    pub code: i32,
    /// 提示消息
    pub message: String,
    /// 服务名称（用于服务标识，可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    /// 功能名称（用于调试追踪，可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    /// 实际业务数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// 响应封装宏 `json_response_wrap!`
///
/// 用于在 Axum handler 中自动处理 `Result` 并生成 `JsonResponse`。
#[macro_export]
macro_rules! json_response_wrap {
    // 全参数模式: code, message, function_name
    (code=$code:expr, message=$msg:expr, function_name=$func_name:expr, $block:block) => {{ $crate::json_response_wrap_impl!($code, $msg, Some($func_name), $block) }};
    // 两参数模式: code, message
    (code=$code:expr, message=$msg:expr, $block:block) => {{ $crate::json_response_wrap_impl!($code, $msg, None, $block) }};
    // 两参数模式: code, function_name
    (code=$code:expr, function_name=$func_name:expr, $block:block) => {{ $crate::json_response_wrap_impl!($code, "操作成功", Some($func_name), $block) }};
    // 两参数模式: message, function_name
    (message=$msg:expr, function_name=$func_name:expr, $block:block) => {{ $crate::json_response_wrap_impl!(200, $msg, Some($func_name), $block) }};
    // 单参数模式: code
    (code=$code:expr, $block:block) => {{ $crate::json_response_wrap_impl!($code, "操作成功", None, $block) }};
    // 单参数模式: message
    (message=$msg:expr, $block:block) => {{ $crate::json_response_wrap_impl!(200, $msg, None, $block) }};
    // 单参数模式: function_name
    (function_name=$func_name:expr, $block:block) => {{ $crate::json_response_wrap_impl!(200, "操作成功", Some($func_name), $block) }};
    // 无参数模式
    ($block:block) => {{ $crate::json_response_wrap_impl!(200, "操作成功", None, $block) }};
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
        let result: Result<_, SimpleAppWebError> = async move $block.await;
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
///    - `service_name` 和 `function_name` (如果存在) 会被填充到响应中。
pub fn process_data<T, S>(
    code: i32,
    message: S,
    function_name: Option<S>,
    result: Result<T, SimpleAppWebError>,
) -> JsonResponse
where
    T: Serialize,
    S: Into<String>,
{
    // 尝试获取服务名称配置
    let service_name: Option<String> = match AppCoreUtil::get_config_value_by_path("app.name") {
        Some(value) => value.as_str().map(|s| s.to_string()),
        None => None,
    };

    match result {
        // 业务逻辑执行成功
        Ok(data) => JsonResponse {
            code,
            message: message.into(),
            service_name,
            function_name: function_name.map(|s| s.into()),
            data: match serde_json::to_value(data) {
                Ok(Value::Null) => None,
                Ok(v) => Some(v),
                Err(e) => {
                    error!("Serialize response data failed: {:?}", e);
                    None
                }
            },
        },
        Err(err) => {
            let function_name_string = function_name.map(|s| s.into());
            // 统一记录错误日志
            if let Some(source) = &err.source {
                // 构建错误链
                let mut chain = String::new();
                let mut current: Option<&dyn std::error::Error> = Some(source.as_ref());
                while let Some(e) = current {
                    if !chain.is_empty() {
                        chain.push_str(" <- ");
                    }
                    chain.push_str(&e.to_string());
                    current = e.source();
                }

                // 记录详细错误（使用 error 级别）
                error!(
                    error_code = err.code(),
                    error_message = %err.message,
                    error_type = std::any::type_name_of_val(source.as_ref()),
                    original_error = ?source,
                    error_chain = %chain,
                    service_name = ?service_name,
                    function_name = ?function_name_string,
                    "Business logic returned an error with source"
                );
            } else {
                // 没有 source，可能是业务错误（如 400），用 warn 级别
                warn!(
                    error_code = err.code(),
                    error_message = %err.message,
                    service_name = ?service_name,
                    function_name = ?function_name_string,
                    "Business logic returned a custom error (no source)"
                );
            }

            // 返回 JSON 响应
            JsonResponse {
                code: err.code(),
                message: err.message(),
                service_name,
                function_name: function_name_string,
                data: err.into_error_data(),
            }
        }
    }
}
