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
use tracing::warn;

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
///
/// 该函数按以下步骤加载配置：
/// 1. 首先加载基础配置文件 application.toml
/// 2. 确定要使用的 profile，优先级：环境变量 APP_PROFILE > 配置文件中的 app.profile 设置
/// 3. 如果指定了 profile，则加载对应的 application-{profile}.toml 文件并与基础配置合并
///
/// # Returns
/// * `Result<Value>` - 成功时返回合并后的配置值，失败时返回错误
fn load_user_config() -> Result<Value> {
    // 加载基础配置文件 application.toml
    let base_config = load_toml_file("application.toml")?;

    // 确定 profile：优先从环境变量获取，如果环境变量不存在或为空，则从基础配置中获取
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

    // 如果存在有效的 profile 名称，则加载对应的 profile 配置文件并合并
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
///
/// # 参数
/// * `filename` - 要加载的 TOML 文件名
///
/// # 返回值
/// * `Result<Value>` - 成功时返回解析后的 TOML 值，失败时返回错误信息
fn load_toml_file(filename: &str) -> Result<Value> {
    let path = get_config_file_path(filename)?;

    // 检查文件是否存在，不存在则返回空表并记录警告
    if !path.exists() {
        warn!("Config file '{}' does not exist", path.display());
        return Ok(Value::Table(Table::new())); // 文件不存在则返回空表
    }

    // 读取文件内容并解析为 TOML 值
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read TOML file '{}'", path.display()))?;
    let value: Value = toml::from_str(&content)
        .with_context(|| format!("Failed to parse TOML file '{}'", path.display()))?;
    Ok(value)
}

/// 获取配置文件路径
///
/// 优先使用 `CONFIG_DIR` 环境变量，否则使用 `./resources/`
///
/// # 参数
/// * `filename` - 配置文件名
///
/// # 返回值
/// * `Ok(PathBuf)` - 配置文件的完整路径
/// * `Err(anyhow::Error)` - 获取当前工作目录失败时的错误
fn get_config_file_path(filename: &str) -> Result<PathBuf> {
    // 优先级 1: 环境变量 CONFIG_DIR
    if let Ok(config_dir) = std::env::var("CONFIG_DIR") {
        return Ok(Path::new(&config_dir).join(filename));
    }

    // 优先级 2: 当前工作目录下的 resources
    let current_dir = std::env::current_dir().context("Failed to get current working directory")?;
    let resource_path = current_dir.join("resources").join(filename);
    if resource_path.exists() {
        return Ok(resource_path);
    }

    // 优先级 3: 与可执行文件同级的 resources 目录 (常见于发布包结构)
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let exe_resource_path = exe_dir.join("resources").join(filename);
            if exe_resource_path.exists() {
                return Ok(exe_resource_path);
            }
        }
    }

    // 默认返回 ./resources/xxx 供后续使用
    Ok(resource_path)
}
