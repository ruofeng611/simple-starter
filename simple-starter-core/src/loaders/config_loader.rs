//! # 配置加载器
//!
//! 负责从文件系统加载 `application.toml` 和 profile 配置，
//! 并与插件默认配置合并后存入 `GLOBAL_CONFIG`。

use crate::AppCoreUtil;
use crate::global_state::GLOBAL_CONFIG;
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use toml::{Table, Value};

/// 加载并设置全局配置
///
/// 流程：
/// 1. 加载用户配置（application.toml）
/// 2. 若指定了 profile，则加载 application-{profile}.toml 并合并
/// 3. 与插件默认配置合并
/// 4. 写入 GLOBAL_CONFIG
pub(crate) fn global_config_load(plugin_default_configs: Value) -> Result<()> {
    let user_config = load_user_config()?;
    let final_config = AppCoreUtil::merge_toml_values(plugin_default_configs, user_config);
    GLOBAL_CONFIG
        .set(final_config)
        .map_err(|_| anyhow!("Failed to set global config"))?;
    Ok(())
}

/// 加载用户配置（支持 profile）
fn load_user_config() -> Result<Value> {
    // Step 1: 加载基础配置
    let base_config = load_toml_file("application.toml")?;

    // Step 2: 确定 profile（优先从环境变量，其次从配置）
    let profile = std::env::var("APP_PROFILE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            base_config
                .get("app")
                .and_then(|app| app.get("profile"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    // Step 3: 若有 profile，则加载 profile 配置并合并
    if let Some(profile_name) = profile {
        if !profile_name.is_empty() {
            let profile_filename = format!("application-{}.toml", profile_name);
            let profile_config = load_toml_file(&profile_filename)?;
            return Ok(AppCoreUtil::merge_toml_values(base_config, profile_config));
        }
    }

    Ok(base_config)
}

/// 从配置目录加载单个 TOML 文件
fn load_toml_file(filename: &str) -> Result<Value> {
    let path = get_config_file_path(filename)?;
    if !path.exists() {
        return Ok(Value::Table(Table::new())); // 文件不存在则返回空表
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read TOML file '{}'", path.display()))?;
    let value: Value = toml::from_str(&content)
        .with_context(|| format!("Failed to parse TOML file '{}'", path.display()))?;
    Ok(value)
}

/// 获取配置文件路径
///
/// 优先使用 `CONFIG_DIR` 环境变量，否则使用 `./resources/`
fn get_config_file_path(filename: &str) -> Result<PathBuf> {
    if let Ok(config_dir) = std::env::var("CONFIG_DIR") {
        return Ok(Path::new(&config_dir).join(filename));
    }
    let current_dir = std::env::current_dir()
        .context("Failed to get current working directory")?
        .join("resources")
        .join(filename);
    Ok(current_dir)
}
