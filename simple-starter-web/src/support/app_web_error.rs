//! Web 应用专用错误类型。
//!
//! 提供统一的错误封装 `SimpleAppWebError`，并实现了从标准 Error 的自动转换。
//! 确保错误发生时，既能向客户端返回标准 JSON，又能向服务端日志记录详细堆栈。

use serde::Serialize;
use serde_json::Value;
use simple_starter_core::tracing::error;

/// Web 应用的标准错误结构。
pub struct SimpleAppWebError {
    /// 业务错误码 (例如 400, 500, 1001)
    pub code: i32,
    /// 对外展示的错误消息
    pub message: String,
    /// 原始错误源（用于服务端日志记录，不返回给前端）
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
    /// 附加数据（可序列化），会包含在响应 JSON 的 `data` 字段中
    pub data: Option<Value>,
}

impl SimpleAppWebError {
    /// 创建一个新的基础错误
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
            data: None,
        }
    }

    /// 附加可序列化的数据到错误中
    pub fn with_data(mut self, data: impl Serialize) -> Self {
        self.data = match serde_json::to_value(data) {
            Ok(Value::Null) => None,
            Ok(v) => Some(v),
            Err(e) => {
                error!("Serialize response data failed: {:?}", e);
                None
            }
        };
        self
    }

    /// 关联原始底层错误（用于日志追踪）
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self
    }

    /// 获取错误码
    pub fn code(&self) -> i32 {
        self.code
    }

    /// 获取错误消息
    pub fn message(&self) -> String {
        self.message.clone()
    }

    /// 消费并获取错误消息
    pub fn into_message(self) -> String {
        self.message
    }

    /// 消费并获取附加数据
    pub fn into_error_data(self) -> Option<Value> {
        self.data
    }
}

/// 实现 From trait，自动将标准错误转换为 Web 错误
///
/// # 行为说明
/// 1. 自动记录完整的错误链日志。
/// 2. 转换为 HTTP 500 "服务器内部错误"，隐藏内部细节防止泄露，但保留 source 用于调试。
impl<E> From<E> for SimpleAppWebError
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: E) -> Self {
        // 1. 构建错误链字符串（用于日志）
        let mut chain = String::new();
        let mut current: Option<&dyn std::error::Error> = Some(&err);
        while let Some(e) = current {
            if !chain.is_empty() {
                chain.push_str(" <- ");
            }
            chain.push_str(&e.to_string());
            current = e.source();
        }

        // 2. 记录详细错误日志
        // 使用 ?err (Debug) 打印原始错误结构，确保细节可见
        error!(
            error_type = std::any::type_name::<E>(),
            original_error = ?err,
            error_chain = %chain,
            "Generic error converted to SimpleAppWebError"
        );

        // 3. 返回通用的 500 错误
        SimpleAppWebError::new(500, "服务器内部错误").with_source(err)
    }
}
