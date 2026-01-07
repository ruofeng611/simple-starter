//! # 核心配置结构定义

use serde::{Deserialize, Serialize};

/// 应用顶层配置结构（对应 TOML 的根）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct AppConfig {
    pub app: CoreConfig,
}

/// 核心配置项（位于 `[app]` 下）
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct CoreConfig {
    pub log_level: String,
    pub profile: Option<String>,
    pub with_thread_id: bool,
    pub with_thread_name: bool,
    pub name: Option<String>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            log_level: "INFO".to_string(),
            profile: None,
            with_thread_id: true,
            with_thread_name: true,
            name: None,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app: CoreConfig::default(),
        }
    }
}
