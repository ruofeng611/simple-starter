//! Web 应用专用错误类型。
//!
//! `SimpleAppWebError` 是该模块内部实现的错误类型，它与 `JsonResponse` 紧密集成。
//! 任何实现了 `std::error::Error` 的错误都可以被自动转换为此类型。

use serde::Serialize;
use serde_json::Value;
use simple_starter_core::tracing::{error, warn};

/// Web 应用的标准错误结构。
///
/// 它不仅包含错误码和消息，还保留了原始错误源（用于日志记录）和可选的附加数据。
pub struct SimpleAppWebError {
    /// 业务错误码。
    pub code: i32,
    /// 错误消息。
    pub message: String,
    /// 原始的底层错误，用于在日志中打印完整的错误链。
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
    /// 可选的附加错误数据，会被序列化到 `JsonResponse` 的 `data` 字段中。
    pub data: Option<Value>,
}

impl SimpleAppWebError {
    /// 创建一个新的错误实例。
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
            data: None,
        }
    }

    /// 为错误附加序列化的数据。
    pub fn with_data(mut self, data: impl Serialize) -> Self {
        self.data = match serde_json::to_value(data) {
            Ok(Value::Null) => None,
            Ok(v) => Some(v),
            Err(e) => {
                warn!("Serialize response data failed: {}", e);
                None
            }
        };
        self
    }

    /// 关联原始错误源。
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self
    }

    /// 获取错误码。
    pub fn code(&self) -> i32 {
        self.code
    }

    /// 获取错误消息（克隆一份）。
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// 消费错误，直接获取消息（避免克隆）。
    pub fn into_message(self) -> String {
        self.message
    }

    /// 消费错误，直接获取附加数据（避免克隆）。
    pub fn into_error_data(self) -> Option<Value> {
        self.data
    }
}

/// 实现 `From` trait，使得任何标准错误都能自动转换为 `SimpleAppWebError`。
///
/// # 转换逻辑
/// 1. **错误链记录**: 会遍历并拼接整个错误链（`e <- e.source() <- ...`），并在日志中记录。
/// 2. **默认值**: 转换后的错误码固定为 `500`，消息为 `"服务器内部错误"`。
/// 3. **源错误保留**: 原始错误会被存储在 `source` 字段中，以备后续排查。
impl<E> From<E> for SimpleAppWebError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: E) -> Self {
        // 构建完整错误链
        let mut chain = String::new();
        let mut current: Option<&dyn std::error::Error> = Some(&err);
        while let Some(e) = current {
            if !chain.is_empty() {
                chain.push_str(" <- ");
            }
            chain.push_str(&e.to_string());
            current = e.source();
        }
        // 记录详细的错误日志
        error!(
            error_type = std::any::type_name::<E>(),
            original_error = %err,
            error_chain = %chain,
            "Generic error converted to SimpleAppWebError"
        );
        // 返回标准化的 500 错误
        SimpleAppWebError::new(500, "服务器内部错误").with_source(err)
    }
}