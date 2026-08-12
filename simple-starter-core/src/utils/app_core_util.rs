use crate::ComponentWrapper;
use crate::core::app_error::{ComponentError, LogExpectExt, TomlConfigError};
use crate::core::app_types::{ComponentKey, DestroyFn};
use crate::global_state::{COMPONENT_REPOSITORY, GLOBAL_CONFIG, INSTANCE_NAMES_BY_TRAIT, TRAIT_OBJ_CACHE};
use crate::loaders::component_loader::COMPONENT_ORDER;
use crate::utils::app_inner_util::get_short_type_name;
use serde::Deserialize;
use std::any::{Any, TypeId};
use std::sync::Arc;
use toml::Value;
use crate::core::app_component::Injectable;

/// 动态注册组件时的顺序策略
///
/// 控制组件在启动顺序列表中的位置，从而影响创建/初始化顺序和销毁顺序。
pub enum ComponentOrder {
    /// 头插入：先创建、先初始化、后销毁。
    /// 适用于基础设施组件（如 HttpTemplate），它们不依赖其他组件，但可能被其他组件依赖。
    Front,
    /// 尾插入：后创建、后初始化、先销毁。
    /// 适用于普通组件或依赖已有组件的动态注册组件。
    Back,
}

impl Default for ComponentOrder {
    fn default() -> Self {
        ComponentOrder::Back
    }
}

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
        order: ComponentOrder,
    ) -> Result<(), ComponentError>
    where
        T: Any + Send + Sync + 'static,
        F: FnOnce(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let name = get_short_type_name::<T>();
        Self::register_component_with_name(component, name, destroy_fn, order)
    }

    /// 动态注册组件（指定名称）
    ///
    /// 在运行时将组件实例注入到仓库中。
    pub fn register_component_with_name<T, S, F, Fut>(
        component: T,
        name: S,
        destroy_fn: Option<F>,
        order: ComponentOrder,
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
            match order {
                ComponentOrder::Front => order_guard.insert(0, key),
                ComponentOrder::Back => order_guard.push(key),
            }
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

    // =========================================================================
    // Trait object 注入 API
    // =========================================================================

    /// 按 trait + name 获取组件 → `Arc<dyn Trait>`
    ///
    /// 从 `TRAIT_OBJ_CACHE` 中取出预计算好的裸指针，
    /// 通过 `Arc::from_raw` 重建 `Arc<dyn Trait>`，
    /// clone 后更新缓存中的指针。
    ///
    /// # Safety
    ///
    /// 内部使用 unsafe 将裸指针重建为 `Arc<dyn Trait>`。
    /// 安全性由以下不变量保证：
    /// - 指针由 `Arc::into_raw` 在 `populate_trait_obj_cache` 中创建
    /// - 指针类型与 `Trait` 泛型参数匹配（由 `TraitImplRegistration` 的 trait_type_id 保证）
    /// - 每次取出后 clone 一份，原指针放回缓存
    pub fn get_component_by_trait_and_name<Trait: Injectable + ?Sized>(
        name: &str,
    ) -> Result<Arc<Trait>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let cache_key = (trait_type_id, name.to_string());

        let arc_injectable = TRAIT_OBJ_CACHE
            .get(&cache_key)
            .ok_or_else(|| ComponentError::TraitImplNotFound {
                trait_name: std::any::type_name::<Trait>().to_string(),
            })?
            .value()
            .clone();

        // 将 Arc<dyn Injectable> 转为裸指针，再通过 transmute_copy 转为目标 trait 的裸指针
        let ptr_injectable: *const dyn Injectable = Arc::into_raw(arc_injectable);
        // SAFETY: *const dyn Injectable 和 *const Trait 都是 fat pointer（128 bits on 64-bit），
        // 二进制布局相同（data_ptr + vtable_ptr）。
        // Trait: Injectable 保证 vtable 兼容 —— dyn Trait 的 vtable 包含了 dyn Injectable 的方法。
        let ptr_trait: *const Trait =
            unsafe { std::mem::transmute_copy::<*const dyn Injectable, *const Trait>(&ptr_injectable) };
        // SAFETY: ptr_trait 由 Arc::into_raw 创建的指针经 transmute 得到，类型匹配
        Ok(unsafe { Arc::from_raw(ptr_trait) })
    }

    /// 按 trait 获取唯一实现 → `Arc<dyn Trait>`
    ///
    /// 从 `INSTANCE_NAMES_BY_TRAIT`（`populate_trait_obj_cache` 时填充）
    /// 直接获取该 trait 的所有实例名，要求恰好一个。
    ///
    /// **不扫描 `COMPONENT_REPOSITORY`**，避免 create 阶段的死锁：
    /// create 持有 `get_mut` 写锁，遍历 `COMPONENT_REPOSITORY.iter()` 会阻塞在同一 shard。
    ///
    /// 错误：
    /// - 0 个实现类型 → `TraitImplNotFound`
    /// - 多个实现类型或实例 → `AmbiguousTraitImpl`
    pub fn get_component_by_trait<Trait: Injectable + ?Sized>(
    ) -> Result<Arc<Trait>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let trait_name = std::any::type_name::<Trait>().to_string();

        let instance_names = INSTANCE_NAMES_BY_TRAIT
            .get(&trait_type_id)
            .ok_or_else(|| ComponentError::TraitImplNotFound {
                trait_name: trait_name.clone(),
            })?;

        let names = instance_names.value();
        if names.is_empty() {
            return Err(ComponentError::TraitImplNotFound { trait_name });
        }
        if names.len() > 1 {
            return Err(ComponentError::AmbiguousTraitImpl {
                trait_name,
                candidates: names.clone(),
            });
        }

        Self::get_component_by_trait_and_name::<Trait>(&names[0])
    }

    /// 按 trait 获取所有实现 → `Vec<Arc<dyn Trait>>`
    ///
    /// 从 `INSTANCE_NAMES_BY_TRAIT` 直接获取该 trait 的所有实例名，
    /// 逐个从 `TRAIT_OBJ_CACHE` 取出。
    ///
    /// **不扫描 `COMPONENT_REPOSITORY`**，避免 create 阶段的死锁。
    ///
    /// 如果 trait 没有实现类型或所有实例都未被缓存，返回空 Vec。
    pub fn get_components_by_trait<Trait: Injectable + ?Sized>(
    ) -> Result<Vec<Arc<Trait>>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let trait_name = std::any::type_name::<Trait>().to_string();

        let instance_names = INSTANCE_NAMES_BY_TRAIT
            .get(&trait_type_id)
            .ok_or_else(|| ComponentError::TraitImplNotFound {
                trait_name: trait_name.clone(),
            })?;

        let mut results = Vec::new();
        for name in instance_names.value().iter() {
            match Self::get_component_by_trait_and_name::<Trait>(name) {
                Ok(entry) => results.push(entry),
                Err(_) => {
                    tracing::debug!(
                        "Trait '{}' instance '{}' not found in cache, skipped",
                        trait_name, name
                    );
                }
            }
        }

        Ok(results)
    }
}
