use crate::ComponentWrapper;
use crate::core::app_error::{ComponentError, LogExpectExt, TomlConfigError};
use crate::core::app_types::{ComponentKey, DestroyFn};
use crate::global_state::{COMPONENT_REPOSITORY, GLOBAL_CONFIG};
use crate::loaders::component_loader::COMPONENT_ORDER;
use crate::utils::app_inner_util::get_short_type_name;
use serde::Deserialize;
use std::any::{Any, TypeId};
use std::sync::Arc;
use toml::Value;

pub struct AppCoreUtil;

impl AppCoreUtil {
    /// 通过点分隔路径获取配置值 (例如 "web.port")
    pub fn get_config_value_by_path(path: &str) -> Option<&'static Value> {
        let keys: Vec<&str> = path.split('.').collect();

        let mut current_ref = GLOBAL_CONFIG.get().log_expect(
            "Global configuration not initialized. Ensure Application::run() is called.",
        );

        for key in keys {
            match current_ref {
                Value::Table(table) => {
                    current_ref = table.get(key)?;
                }
                _ => return None,
            }
        }
        Some(current_ref)
    }

    /// 获取配置并反序列化为结构体
    pub fn get_config_to_struct<T>(path: &str) -> Result<T, TomlConfigError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value =
            Self::get_config_value_by_path(path).ok_or_else(|| TomlConfigError::PathNotFound {
                path: path.to_string(),
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

    /// 动态注册组件，使用类型名作为名称
    pub fn register_component<T, F, Fut>(
        component: T,
        destroy_fn: Option<F>,
    ) -> Result<(), ComponentError>
    where
        T: Any + Send + Sync + 'static,
        F: FnOnce(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let name = get_short_type_name::<T>();
        Self::register_component_with_name(component, name, destroy_fn)
    }

    /// 动态注册组件（指定名称）
    ///
    /// 在运行时将组件实例注入到仓库中。
    pub fn register_component_with_name<T, S, F, Fut>(
        component: T,
        name: S,
        destroy_fn: Option<F>,
    ) -> Result<(), ComponentError>
    where
        T: Any + Send + Sync + 'static,
        S: Into<String>,
        F: FnOnce(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let name_str = name.into();
        let type_id = TypeId::of::<T>();
        let key: ComponentKey = (type_id, name_str.clone());

        // 1. 检查是否重复
        if COMPONENT_REPOSITORY.contains_key(&key) {
            return Err(ComponentError::AlreadyExists {
                type_name: std::any::type_name::<T>().to_string(),
                name: name_str,
            });
        }

        // 2. 封装 Destroy 函数
        let destroy_fn: Option<DestroyFn<T>> = if let Some(destroy_fn) = destroy_fn {
            Some(Box::new(move |component| Box::pin(destroy_fn(component))))
        } else {
            None
        };

        // 3. 创建 Wrapper
        let component_warper = ComponentWrapper {
            create_fn: None,
            init_fn: None,
            destroy_fn,
            inner: Some(Arc::new(component)),
        };
        COMPONENT_REPOSITORY.insert(key.clone(), Box::new(component_warper));

        // 4. 记录顺序以便销毁
        {
            let mut order_guard =
                COMPONENT_ORDER
                    .lock()
                    .map_err(|_| ComponentError::InternalError {
                        message: "Failed to lock component order".to_string(),
                    })?;
            order_guard.push(key);
        }

        Ok(())
    }

    /// 获取组件实例（按类型）
    pub fn get_component<T>() -> Result<Arc<T>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
    {
        let name = get_short_type_name::<T>();
        Self::get_component_by_name(name)
    }

    /// 获取组件实例（按名称）
    pub fn get_component_by_name<T, S>(name: S) -> Result<Arc<T>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
        S: Into<String>,
    {
        let name_str = name.into();
        let type_id = TypeId::of::<T>();
        let key: ComponentKey = (type_id, name_str.clone());

        if let Some(entry) = COMPONENT_REPOSITORY.get(&key) {
            let processor = entry.value();
            let as_any = processor.as_any();

            if let Some(wrapper) = as_any.downcast_ref::<ComponentWrapper<T>>() {
                if let Some(ref inner) = wrapper.inner {
                    Ok(inner.clone())
                } else {
                    Err(ComponentError::NotInitialized {
                        type_name: std::any::type_name::<T>().to_string(),
                        name: name_str,
                    })
                }
            } else {
                Err(ComponentError::DowncastFailed {
                    type_name: std::any::type_name::<T>().to_string(),
                    name: name_str,
                })
            }
        } else {
            Err(ComponentError::NotFound {
                type_name: std::any::type_name::<T>().to_string(),
                name: name_str,
            })
        }
    }

    /// 获取某类型的所有组件
    pub fn get_component_by_type<T>() -> Result<Vec<Arc<T>>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
    {
        let target_type_id = TypeId::of::<T>();
        let mut results = Vec::new();

        for entry in COMPONENT_REPOSITORY.iter() {
            let ((type_id, name), processor) = entry.pair();

            if *type_id == target_type_id {
                if let Some(wrapper) = processor.as_any().downcast_ref::<ComponentWrapper<T>>() {
                    if let Some(ref inner) = wrapper.inner {
                        results.push(inner.clone());
                    } else {
                        return Err(ComponentError::NotInitialized {
                            type_name: std::any::type_name::<T>().to_string(),
                            name: name.clone(),
                        });
                    }
                } else {
                    return Err(ComponentError::DowncastFailed {
                        type_name: std::any::type_name::<T>().to_string(),
                        name: name.clone(),
                    });
                }
            }
        }

        Ok(results)
    }
}
