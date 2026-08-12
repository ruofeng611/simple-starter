use thiserror::Error;
use tracing;

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

/// 组件系统相关的特定错误
#[derive(Debug, Error)]
pub enum ComponentError {
    #[error("Component not found for type: {type_name}, name: {name}")]
    NotFound { type_name: String, name: String },

    #[error("Failed to downcast component: {type_name}, name: {name}")]
    DowncastFailed { type_name: String, name: String },

    #[error("Component already exists for type: {type_name}, name: {name}")]
    AlreadyExists { type_name: String, name: String },

    #[error("Component not initialized (inner is None) for type: {type_name}, name: {name}")]
    NotInitialized { type_name: String, name: String },

    #[error("Internal error: {message}")]
    InternalError { message: String },

    #[error("No implementation found for trait: {trait_name}")]
    TraitImplNotFound { trait_name: String },

    #[error("Ambiguous trait implementation for '{trait_name}': candidates = {candidates:?}")]
    AmbiguousTraitImpl {
        trait_name: String,
        candidates: Vec<String>,
    },
}

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
