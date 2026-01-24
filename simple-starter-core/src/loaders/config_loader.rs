//! # 配置加载器
//!
//! 负责从文件系统加载 `application.toml` 和 profile 配置，
//! 并与默认配置合并后存入 `GLOBAL_CONFIG`。

use crate::global_state::GLOBAL_CONFIG;
use crate::utils::app_inner_util::merge_toml_values;
use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::PathBuf;
use toml::Value;

/// 加载并设置全局配置
///
/// 流程：
/// 1. 加载用户配置（application.toml）。
/// 2. 若指定了 profile，则加载 application-{profile}.toml 并合并。
/// 3. 将用户配置合并到 Application 的默认配置之上。
/// 4. 写入 GLOBAL_CONFIG。
pub(crate) fn global_config_load(default_config: Value) -> Result<()> {
    let user_config = load_user_config()?;
    // 注意：merge_toml_values(base, overlay)，这里 base 是默认配置，overlay 是用户文件配置
    let final_config = merge_toml_values(default_config, user_config);

    GLOBAL_CONFIG
        .set(final_config)
        .map_err(|_| anyhow!("Failed to set global config"))?;
    Ok(())
}

/// 加载用户配置文件
fn load_user_config() -> Result<Value> {
    // 1. 加载基础配置 application.toml
    let base_config = load_toml_file("application.toml")?;

    // 2. 确定 Profile (优先级: 环境变量 > 配置文件)
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

    // 3. 如果有 Profile，加载对应文件并合并
    if let Some(profile_name) = profile {
        if !profile_name.is_empty() {
            let profile_filename = format!("application-{}.toml", profile_name);
            let profile_config = load_toml_file(&profile_filename)?;
            return Ok(merge_toml_values(base_config, profile_config));
        }
    }

    Ok(base_config)
}

/// 从标准路径加载单个 TOML 文件
///
/// 查找顺序：
/// 1. `CONFIG_DIR` 环境变量
/// 2. `./resources/`
/// 3. `<exe_dir>/resources/`
fn load_toml_file(filename: &str) -> Result<Value> {
    let mut candidate_paths = Vec::new();

    // 1. 检查环境变量
    if let Ok(config_dir) = std::env::var("CONFIG_DIR") {
        candidate_paths.push(PathBuf::from(config_dir).join(filename));
    }

    // 2. 检查当前工作目录
    if let Ok(current_dir) = std::env::current_dir() {
        candidate_paths.push(current_dir.join("resources").join(filename));
    }

    // 3. 检查可执行文件目录
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            candidate_paths.push(exe_dir.join("resources").join(filename));
        }
    }

    // 4. 遍历查找
    let path = candidate_paths.into_iter().find(|p| p.exists());

    match path {
        Some(ref p) => {
            let content = fs::read_to_string(p)
                .with_context(|| format!("Failed to read TOML file '{}'", p.display()))?;
            let value: Value = toml::from_str(&content)
                .with_context(|| format!("Failed to parse TOML file '{}'", p.display()))?;
            Ok(value)
        }
        None => {
            // 文件不存在视为正常，返回空配置
            Ok(Value::Table(toml::value::Table::new()))
        }
    }
}
