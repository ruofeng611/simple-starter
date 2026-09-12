//! # 核心工具（用户可用的公开工具）
//!
//! 公开层：点分路径配置查询与反序列化函数、配置错误类型、日志版 expect 扩展。
//! 供用户代码、组件条件评估、插件与 `AppContext` 共享使用。

use serde::Deserialize;
use thiserror::Error;
use toml::Value;

// =============================================================================
// 日志版 expect 扩展
// =============================================================================

/// 扩展 Trait：提供“日志版” expect 行为
///
/// 用于在无法恢复的致命错误发生时，先打印详细的 Error 日志，然后安全退出或 Panic。
/// 这比直接 unwrap/expect 更好，因为能在日志文件中留下痕迹。
pub trait LogExpectExt<T> {
    /// 如果成功则返回内部值 T；
    /// 如果失败则打印 error 日志（包含 Debug 信息）并 panic。
    ///
    /// # 参数
    /// * `msg` - 错误上下文描述
    fn log_expect(self, msg: &str) -> T;
}

// 为 Result 实现
impl<T, E: std::fmt::Debug> LogExpectExt<T> for Result<T, E> {
    fn log_expect(self, msg: &str) -> T {
        match self {
            Ok(val) => val,
            Err(e) => {
                // 使用 {:?} 打印错误，确保输出完整的错误堆栈/上下文
                tracing::error!("{}: {:?}", msg, e);
                panic!("{}: {:?}", msg, e);
            }
        }
    }
}

// 为 Option 实现
impl<T> LogExpectExt<T> for Option<T> {
    fn log_expect(self, msg: &str) -> T {
        match self {
            Some(val) => val,
            None => {
                tracing::error!("{}", msg);
                panic!("{}", msg);
            }
        }
    }
}

// =============================================================================
// 配置错误
// =============================================================================

/// 配置加载相关的特定错误
#[derive(Debug, Error)]
pub enum TomlConfigError {
    #[error("Configuration path '{path}' not found (missing key)")]
    PathNotFound { path: String },

    #[error("Failed to convert TOML to JSON for path '{path}': {source}")]
    TomlToJsonConversionFailed {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("Failed to deserialize config at path '{path}': {source}")]
    DeserializationFailed {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

// =============================================================================
// 配置查询
// =============================================================================

/// 按点分隔路径在配置树中查找值（例如 "web.port"）
///
/// 配置查询的统一入口：组件条件评估、插件配置读取、用户代码均经此函数。
pub fn get_config_value_by_path<'a>(config: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current_ref = config;

    for key in path.split('.') {
        match current_ref {
            Value::Table(table) => {
                current_ref = table.get(key)?;
            }
            _ => return None,
        }
    }
    Some(current_ref)
}

/// 从配置树中反序列化指定路径为结构体
pub fn get_config_to_struct<T>(config: &Value, path: &str) -> Result<T, TomlConfigError>
where
    T: for<'de> Deserialize<'de>,
{
    let value = get_config_value_by_path(config, path).ok_or_else(|| {
        TomlConfigError::PathNotFound {
            path: path.to_string(),
        }
    })?;

    let json_value = serde_json::to_value(value).map_err(|source| {
        TomlConfigError::TomlToJsonConversionFailed {
            path: path.to_string(),
            source,
        }
    })?;

    serde_json::from_value(json_value).map_err(|source| {
        TomlConfigError::DeserializationFailed {
            path: path.to_string(),
            source,
        }
    })
}
