//! # 核心工具类
//!
//! 提供配置读取、组件注册与获取、TOML 合并等通用功能。

use crate::core::app_error::AppCoreError;
use crate::global_state::{ComponentKey, COMPONENT_REPOSITORY, GLOBAL_CONFIG};
use dashmap::Entry;
use serde::Deserialize;
use std::any::{type_name, Any, TypeId};
use std::sync::{Arc, RwLock};
use toml::Value;

/// 核心工具集合（仅含静态方法）
pub struct AppCoreUtil;

impl AppCoreUtil {
    /// 根据点分路径从全局配置中获取 TOML 值
    ///
    /// # 示例
    /// ```toml
    /// [app]
    /// log_level = "DEBUG"
    /// ```
    /// ```rust,ignore
    /// let level = AppCoreUtil::get_config_value_by_path("app.log_level");
    /// ```
    pub fn get_config_value_by_path(path: &str) -> Result<&'static Value, AppCoreError> {
        let keys: Vec<&str> = path.split('.').collect();
        let current = GLOBAL_CONFIG
            .get()
            .ok_or(AppCoreError::ConfigNotInitialized)?;
        let mut current_ref = current;
        for key in keys {
            match current_ref {
                Value::Table(table) => {
                    if let Some(next) = table.get(key) {
                        current_ref = next;
                    } else {
                        return Err(AppCoreError::ConfigPathNotFound {
                            path: path.to_string(),
                            key: key.to_string(),
                        });
                    }
                }
                _ => {
                    return Err(AppCoreError::ConfigPathNotTable {
                        path: path.to_string(),
                        found: current_ref.clone(),
                    });
                }
            }
        }
        Ok(current_ref)
    }

    /// 将配置路径下的值反序列化为指定结构体
    ///
    /// # 示例
    /// ```rust,ignore
    /// #[derive(Deserialize)]
    /// struct DbConfig { url: String }
    ///
    /// let db: DbConfig = AppCoreUtil::get_config_to_struct("database").unwrap();
    /// ```
    pub fn get_config_to_struct<T>(path: &str) -> Result<T, AppCoreError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = Self::get_config_value_by_path(path)?;
        let json_value = serde_json::to_value(value)
            .map_err(|source| AppCoreError::TomlToJsonConversionFailed {
                path: path.to_string(),
                source,
            })?;
        serde_json::from_value(json_value)
            .map_err(|source| AppCoreError::ConfigDeserializationFailed {
                path: path.to_string(),
                source,
            })
    }

    /// 注册一个组件（使用类型短名作为组件名）
    ///
    /// 组件名 = 类型全名中最后一个 `::` 后的部分（如 `MyService`）
    pub fn register_component<T: Any + Send + Sync>(component: T) -> Result<(), AppCoreError> {
        let full = type_name::<T>();
        let short_name = if let Some(pos) = full.rfind("::") {
            full[pos + 2..].to_string()
        } else {
            full.to_string()
        };
        Self::register_component_with_name(short_name, component)
    }

    /// 使用自定义名称注册组件
    ///
    /// 若已存在同类型+同名组件，返回错误。
    pub fn register_component_with_name<T: Any + Send + Sync>(
        name: impl Into<String>,
        component: T,
    ) -> Result<(), AppCoreError> {
        let name = name.into();
        let key: ComponentKey = (TypeId::of::<T>(), name.clone());
        let value: Arc<RwLock<T>> = Arc::new(RwLock::new(component));
        // 使用 entry API 原子性地检查并插入
        match COMPONENT_REPOSITORY.entry(key.clone()) {
            Entry::Occupied(_) => {
                return Err(AppCoreError::ComponentAlreadyRegistered {
                    type_id: TypeId::of::<T>(),
                    name,
                });
            }
            Entry::Vacant(vacant) => {
                vacant.insert(value as Arc<dyn Any + Send + Sync>);
            }
        }
        Ok(())
    }

    /// 按名称获取指定类型的组件
    ///
    /// 返回 `Arc<RwLock<T>>`，支持并发读写。
    pub fn get_component_by_name<T: Any + Send + Sync>(name: &str) -> Result<Arc<RwLock<T>>, AppCoreError> {
        let key: ComponentKey = (TypeId::of::<T>(), name.to_string());
        let value = COMPONENT_REPOSITORY
            .get(&key)
            .ok_or_else(|| AppCoreError::ComponentNotFound {
                type_id: TypeId::of::<T>(),
                name: name.to_string(),
            })?;
        // DashMap::get 返回的是 Ref，需解引用
        Arc::downcast(value.value().clone()).map_err(|_| {
            AppCoreError::ComponentTypeCastFailed {
                name: name.to_string(),
                expected_type: TypeId::of::<T>(),
            }
        })
    }

    /// 获取所有指定类型的组件（不区分名称）
    pub fn get_components_by_type<T: Any + Send + Sync>() -> Result<Vec<Arc<RwLock<T>>>, AppCoreError> {
        let type_id = TypeId::of::<T>();
        let mut results = Vec::new();
        for entry in COMPONENT_REPOSITORY.iter() {
            let (key, value) = (entry.key(), entry.value());
            if key.0 == type_id {
                match Arc::downcast::<RwLock<T>>(value.clone()) {
                    Ok(casted) => results.push(casted),
                    Err(_) => {
                        return Err(AppCoreError::ComponentTypeCastByKeyFailed {
                            key: key.clone(),
                            expected_type: type_id,
                        });
                    }
                }
            }
        }
        Ok(results)
    }

    /// 递归合并两个 TOML 值（overlay 覆盖 base）
    ///
    /// 仅对 Table 类型进行深度合并，其他类型直接覆盖。
    pub fn merge_toml_values(mut base: Value, overlay: Value) -> Value {
        match (base.as_table_mut(), overlay.as_table()) {
            (Some(base_map), Some(overlay_map)) => {
                for (key, overlay_val) in overlay_map {
                    match base_map.get(key) {
                        Some(base_val) => {
                            let merged =
                                Self::merge_toml_values(base_val.clone(), overlay_val.clone());
                            base_map.insert(key.clone(), merged);
                        }
                        None => {
                            base_map.insert(key.clone(), overlay_val.clone());
                        }
                    }
                }
                base
            }
            _ => overlay,
        }
    }
}
