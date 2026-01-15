//! # 核心工具模块的专用错误类型
//!
//! `AppCoreError` 是 `AppCoreUtil` 工具类中所有操作可能返回的结构化错误。
//! 该枚举使用 `thiserror::Error` 派生，支持：
//! - 用户友好的错误消息（通过 `Display`）
//! - 错误链追踪（通过 `#[source]`）
//! - 轻松转换为其他错误类型（如项目主错误或 `anyhow::Error`）
//!
//! 所有变体均携带足够的上下文信息，便于日志记录、调试或向用户反馈。

use thiserror::Error;
use toml::Value;
use std::any::TypeId;
use crate::global_state::ComponentKey;

/// 核心工具操作可能发生的各类错误。
///
/// 此错误类型覆盖以下功能场景：
/// - 全局配置读取与解析
/// - 组件注册与获取
/// - 内部状态锁竞争失败
///
/// 使用者可通过 `match` 进行细粒度处理，或直接通过 `to_string()` 获取完整错误描述。
#[derive(Error, Debug)]
pub enum AppCoreError {
    /// 全局配置尚未初始化。
    ///
    /// **触发场景**：在调用 `AppCoreUtil::get_config_value_by_path` 或相关方法前，
    /// 未通过 `GLOBAL_CONFIG.set(...)` 初始化配置。
    #[error("Global configuration not initialized")]
    ConfigNotInitialized,

    /// 配置路径不存在，指定的键在 TOML 表中缺失。
    ///
    /// **触发场景**：尝试访问如 `"app.log_level"`，但 `app` 表中无 `log_level` 字段。
    ///
    /// **字段说明**：
    /// - `path`: 完整的点分路径（如 `"database.host"`）
    /// - `key`: 缺失的具体键名（如 `"host"`）
    #[error("Configuration path '{path}' not found (missing key '{key}')")]
    ConfigPathNotFound { path: String, key: String },

    /// 配置路径指向的值不是表（Table），无法继续深入访问。
    ///
    /// **触发场景**：路径如 `"app.log_level.debug"`，但 `log_level` 是字符串而非表。
    ///
    /// **字段说明**：
    /// - `path`: 请求的完整路径
    /// - `found`: 实际遇到的 TOML 值（如 `Value::String("INFO")`）
    #[error("Configuration path '{path}' is not a table (found: {found:?})")]
    ConfigPathNotTable { path: String, found: Value },

    /// 将 TOML 值转换为 JSON 失败。
    ///
    /// **触发场景**：内部使用 `serde_json::to_value` 转换 TOML 以支持反序列化时出错。
    /// 虽然 TOML 和 JSON 类型高度兼容，但在极端情况下（如非 UTF-8 字符串）可能失败。
    ///
    /// **字段说明**：
    /// - `path`: 配置路径
    /// - `source`: 底层 `serde_json::Error`
    #[error("Failed to convert TOML to JSON for path '{path}': {source}")]
    TomlToJsonConversionFailed {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    /// 从配置值反序列化为目标结构体失败。
    ///
    /// **触发场景**：调用 `AppCoreUtil::get_config_to_struct::<T>(path)` 时，
    /// 配置内容与目标类型 `T` 的字段不匹配（如类型错误、缺失必填字段等）。
    ///
    /// **字段说明**：
    /// - `path`: 配置路径
    /// - `source`: 底层 `serde_json::Error`（因内部经 JSON 中转）
    #[error("Failed to deserialize config at path '{path}': {source}")]
    ConfigDeserializationFailed {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    /// 尝试注册已存在的组件（类型 + 名称组合重复）。
    ///
    /// **触发场景**：两次调用 `register_component_with_name::<T>("name")` 使用相同类型和名称。
    ///
    /// **字段说明**：
    /// - `type_id`: 组件的 `TypeId`（运行时类型标识）
    /// - `name`: 注册时使用的组件名称
    #[error("Component with type {type_id:?} and name '{name}' already registered")]
    ComponentAlreadyRegistered { type_id: TypeId, name: String },

    /// 请求的组件未找到。
    ///
    /// **触发场景**：调用 `get_component_by_name::<T>("name")`，但未注册过该类型+名称的组件。
    ///
    /// **字段说明**：
    /// - `type_id`: 请求的组件类型 `TypeId`
    /// - `name`: 请求的组件名称
    #[error("Component with type {type_id:?} and name '{name}' not found")]
    ComponentNotFound { type_id: TypeId, name: String },

    /// 组件类型转换失败（按名称获取时）。
    ///
    /// **触发场景**：仓库中存在同名组件，但其实际类型与请求的泛型 `T` 不匹配。
    /// 通常因注册和获取时使用了不同泛型参数导致。
    ///
    /// **字段说明**：
    /// - `name`: 组件名称
    /// - `expected_type`: 期望的 `TypeId`
    #[error("Type cast failed for component '{name}'. Actual type does not match requested type {expected_type:?}. Please check if the generic type parameter T in get_component<T> matches the registered type.")]
    ComponentTypeCastFailed { name: String, expected_type: TypeId },

    /// 组件类型转换失败（按类型列举时）。
    ///
    /// **触发场景**：在 `get_components_by_type::<T>()` 中，某个匹配 `TypeId` 的组件
    /// 无法 downcast 为 `<T>`，表明内部存储不一致（理论上不应发生）。
    ///
    /// **字段说明**：
    /// - `key`: 组件键 `(TypeId, name)`
    /// - `expected_type`: 期望的 `TypeId`
    #[error("Type cast failed for component with key {key:?} - expected type {expected_type:?}")]
    ComponentTypeCastByKeyFailed {
        key: ComponentKey,
        expected_type: TypeId,
    },
}