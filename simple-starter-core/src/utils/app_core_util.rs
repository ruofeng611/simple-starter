//! # 核心工具类
//!
//! 提供配置读取、组件注册与获取、TOML 合并等通用功能。

use crate::core::app_error::AppCoreError;
use crate::global_state::{ComponentKey, COMPONENT_REPOSITORY, GLOBAL_CONFIG};
use dashmap::Entry;
use serde::Deserialize;
use std::any::{type_name, Any, TypeId};
use std::sync::Arc;
use toml::Value;

/// 核心工具集合（仅含静态方法）
pub struct AppCoreUtil;

impl AppCoreUtil {
    /// 根据点分路径从全局配置中获取 TOML 值
    ///
    /// # 参数
    /// * `path` - 点分路径字符串，用于指定在 TOML 配置中的查找路径
    ///
    /// # 返回值
    /// * `Result<&'static Value, AppCoreError>` - 成功时返回指向静态生命周期的 TOML 值引用，失败时返回应用核心错误
    ///
    /// # 错误
    /// 可能返回以下错误：
    /// - `AppCoreError::ConfigNotInitialized` - 全局配置未初始化
    /// - `AppCoreError::ConfigPathNotFound` - 指定路径在配置中不存在
    /// - `AppCoreError::ConfigPathNotTable` - 路径中的某个节点不是表类型但尝试访问其子键
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
        // 将路径按点分割为多个键
        let keys: Vec<&str> = path.split('.').collect();
        // 获取全局配置的引用
        let current = GLOBAL_CONFIG
            .get()
            .ok_or(AppCoreError::ConfigNotInitialized)?;
        let mut current_ref = current;
        // 遍历路径中的每个键，逐层深入配置结构
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
    /// 此方法内部涉及 TOML 到 JSON 的转换，开销较大。
    /// 请仅在应用启动、插件初始化或低频配置变更时使用，**不要在请求处理的热点路径中调用**。
    ///
    /// # 参数
    /// * `path` - 配置项的路径字符串
    ///
    /// # 返回值
    /// * `Result<T, AppCoreError>` - 成功时返回反序列化后的结构体实例，失败时返回AppCoreError错误
    ///
    /// # 类型参数
    /// * `T` - 实现Deserialize trait的目标结构体类型
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
        // 获取指定路径下的配置值
        let value = Self::get_config_value_by_path(path)?;
        // 将配置值转换为JSON Value格式
        let json_value = serde_json::to_value(value)
            .map_err(|source| AppCoreError::TomlToJsonConversionFailed {
                path: path.to_string(),
                source,
            })?;
        // 从JSON Value反序列化为目标结构体类型
        serde_json::from_value(json_value)
            .map_err(|source| AppCoreError::ConfigDeserializationFailed {
                path: path.to_string(),
                source,
            })
    }

    /// 注册一个组件（使用类型短名作为组件名）
    ///
    /// 组件名 = 类型全名中最后一个 `::` 后的部分（如 `MyService`）
    ///
    /// # 参数
    /// * `component`: 要注册的组件实例，需要实现 Any + Send + Sync trait
    ///
    /// # 返回值
    /// * `Result<(), AppCoreError>`: 注册成功返回 Ok(())，失败返回 AppCoreError
    ///
    /// # 类型约束
    /// * `T: Any + Send + Sync`: 组件类型必须实现 Any、Send 和 Sync trait
    pub fn register_component<T: Any + Send + Sync>(component: T) -> Result<(), AppCoreError> {
        // 提取类型全名
        let full = type_name::<T>();
        // 从类型全名中提取短名称（最后一个 :: 后的部分）
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
    ///
    /// # 参数
    /// * `name` - 组件的名称，可接受任何可以转换为String的类型
    /// * `component` - 要注册的组件实例，必须实现Any + Send + Sync trait
    ///
    /// # 返回值
    /// * `Ok(())` - 注册成功
    /// * `Err(AppCoreError)` - 注册失败，通常是同类型同名组件已存在
    ///
    /// # 类型约束
    /// * `T: Any + Send + Sync` - 组件类型必须能够被任意类型转换，并且支持线程安全共享
    pub fn register_component_with_name<T: Any + Send + Sync>(
        name: impl Into<String>,
        component: T,
    ) -> Result<(), AppCoreError> {
        let name = name.into();
        let key: ComponentKey = (TypeId::of::<T>(), name.clone());
        let value: Arc<T> = Arc::new(component);
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
    /// # 参数
    /// * `name` - 组件名称，用于在组件仓库中查找对应的组件
    ///
    /// # 类型参数
    /// * `T` - 要获取的组件类型，必须实现 `Any + Send + Sync` trait
    ///
    /// # 返回值
    /// * `Result<Arc<T>, AppCoreError>` - 成功时返回包装在 `Arc` 中的组件实例，失败时返回相应的错误
    ///   - `Ok(Arc<T>)`: 成功获取到指定类型的组件
    ///   - `Err(AppCoreError)`: 获取失败，可能的原因包括组件不存在或类型转换失败
    ///
    /// # 错误
    /// * `AppCoreError::ComponentNotFound`: 当指定名称和类型的组件不存在时
    /// * `AppCoreError::ComponentTypeCastFailed`: 当找到组件但类型转换失败时
    ///
    /// # 功能说明
    /// 该函数通过组件类型和名称构建唯一键，在全局组件仓库中查找对应组件，
    /// 并将查找到的组件向下转换为目标类型，返回线程安全的引用计数指针。
    /// 返回 `Arc<T>`，支持并发读。
    pub fn get_component_by_name<T: Any + Send + Sync>(name: &str) -> Result<Arc<T>, AppCoreError> {
        // 构建组件查找键，由组件类型ID和名称组成
        let key: ComponentKey = (TypeId::of::<T>(), name.to_string());
        // 从组件仓库获取组件值
        let value = COMPONENT_REPOSITORY
            .get(&key)
            .ok_or_else(|| AppCoreError::ComponentNotFound {
                type_id: TypeId::of::<T>(),
                name: name.to_string(),
            })?;
        // 将获取到的值进行类型转换，从 Arc<dyn Any + Send + Sync> 转换为 Arc<T>
        Arc::downcast::<T>(value.value().clone()).map_err(|_| {
            AppCoreError::ComponentTypeCastFailed {
                name: name.to_string(),
                expected_type: TypeId::of::<T>(),
            }
        })
    }

    /// 获取所有指定类型的组件（不区分名称）
    ///
    /// # 类型参数
    /// * `T` - 实现了 Any + Send + Sync trait 的组件类型
    ///
    /// # 返回值
    /// * `Result<Vec<Arc<T>>, AppCoreError>` - 成功时返回指定类型组件的Arc指针向量，失败时返回AppCoreError错误
    ///
    /// # 错误
    /// 当组件类型转换失败时，返回 ComponentTypeCastByKeyFailed 错误
    pub fn get_components_by_type<T: Any + Send + Sync>() -> Result<Vec<Arc<T>>, AppCoreError> {
        // 获取目标类型的TypeId用于后续比较
        let type_id = TypeId::of::<T>();
        let mut results = Vec::new();

        // 遍历组件仓库中的所有条目
        for entry in COMPONENT_REPOSITORY.iter() {
            let (key, value) = (entry.key(), entry.value());

            // 检查当前条目的类型ID是否与目标类型匹配
            if key.0 == type_id {
                match Arc::downcast::<T>(value.clone()) {
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
    ///
    /// # 参数
    /// * `base` - 基础 TOML 值，将被合并到此值中
    /// * `overlay` - 覆盖层 TOML 值，其内容将覆盖或合并到基础值中
    ///
    /// # 返回值
    /// 返回合并后的 TOML 值
    pub fn merge_toml_values(mut base: Value, overlay: Value) -> Value {
        // 匹配两个值是否都是表类型，如果是则进行深度合并
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
            // 如果任一值不是表类型，则直接用 overlay 覆盖 base
            _ => overlay,
        }
    }
}
