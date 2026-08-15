use crate::ComponentWrapper;
use crate::core::app_error::{ComponentError, LogExpectExt, TomlConfigError};
use crate::global_state::{
    COMPONENT_REPOSITORY, GLOBAL_CONFIG, PRIMARY_BY_TYPE, TRAIT_OBJ_CACHE,
};
use crate::utils::app_inner_util::{
    get_component_names_by_type, get_impl_component_names_by_trait, get_short_type_name,
};
use serde::Deserialize;
use std::any::{Any, TypeId};
use std::sync::Arc;
use toml::Value;
use crate::core::app_component::{ComponentProcessor, Injectable};

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

    /// 获取组件实例（按类型）
    ///
    /// 先按类型短名（默认组件名）快速查找；未命中时（组件自定义名称）
    /// 按具体类型收集全部实例名：
    /// - 恰好一个 → 返回该实例
    /// - 多个 → `AmbiguousComponent`
    /// - 零个 → `NotFound`
    pub fn get_component<T>() -> Result<Arc<T>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
    {
        let short_name = get_short_type_name::<T>();

        // 1. 快速路径：默认命名（类型短名）直接命中
        if COMPONENT_REPOSITORY.contains_key(&short_name) {
            return Self::get_component_by_name(short_name);
        }

        // 2. 兜底：组件自定义了名称，按具体类型收集全部实例名
        let names = get_component_names_by_type(TypeId::of::<T>());
        match names.as_slice() {
            [] => Err(ComponentError::NotFound {
                type_name: std::any::type_name::<T>().to_string(),
                name: short_name,
            }),
            [name] => Self::get_component_by_name(name.clone()),
            _ => Err(ComponentError::AmbiguousComponent {
                type_name: std::any::type_name::<T>().to_string(),
                candidates: names,
            }),
        }
    }

    /// 获取首要（primary）实例（按类型）
    ///
    /// 供插件方等"不知道用户会取什么名、必须按类型获取"的场景使用：
    /// 1. 该类型注册了 primary → 直接返回 primary 实例
    /// 2. 未注册 primary → 回退为 `get_component` 语义（默认名快速路径 + 按类型唯一实例）
    pub fn get_primary_component<T>() -> Result<Arc<T>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
    {
        let type_id = TypeId::of::<T>();

        // 1. primary 优先：该类型注册了首要实例，直接返回
        if let Some(primary_name) = PRIMARY_BY_TYPE.get(&type_id) {
            return Self::get_component_by_name(primary_name.value().clone());
        }

        // 2. 回退：与 get_component 一致的默认名快速路径 + 类型唯一性兜底
        Self::get_component::<T>()
    }

    /// 获取组件实例（按名称）
    pub fn get_component_by_name<T, S>(name: S) -> Result<Arc<T>, ComponentError>
    where
        T: Any + Send + Sync + 'static,
        S: Into<String>,
    {
        let name_str = name.into();

        if let Some(entry) = COMPONENT_REPOSITORY.get(&name_str) {
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
            let (name, processor) = entry.pair();

            // 类型信息取自处理器本身（`ComponentProcessor::type_id`）；完全限定语法消除 Any::type_id 歧义
            if ComponentProcessor::type_id(&**processor) == target_type_id {
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
    /// 从 `TRAIT_OBJ_CACHE` 取出 `TraitObjectEntry`，用注册时记录的 dyn Trait
    /// 真实 vtable 与实例数据指针拼回 fat pointer，重建 `Arc<dyn Trait>`。
    /// vtable 是 accessor 内 coercion 时编译器算出的真实值，与缓存条目同源
    /// （同一注册条目的 trait_type_id + accessor）。
    ///
    /// # Safety
    ///
    /// 内部使用 unsafe 将拆解后的 fat pointer 位重新拼回 `Arc<dyn Trait>`。
    /// 安全性由以下不变量保证：
    /// - data 指针由 `Arc::into_raw` 在取用侧创建（指向 ArcInner）
    /// - vtable 是注册时 coercion 生成的 dyn Trait 真实 vtable（'static 只读静态数据），
    ///   其 [drop, size, align] 头三槽与该具体类型一致，`Arc::from_raw` 的 drop 行为正确
    pub fn get_component_by_trait_and_name<Trait: Injectable + ?Sized>(
        name: &str,
    ) -> Result<Arc<Trait>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let cache_key = (trait_type_id, name.to_string());

        let entry = TRAIT_OBJ_CACHE
            .get(&cache_key)
            .ok_or_else(|| ComponentError::TraitImplNotFound {
                trait_name: std::any::type_name::<Trait>().to_string(),
            })?
            .value()
            .clone();

        // 拆出 data 指针（ArcInner），与注册时记录的 dyn Trait 真实 vtable 拼回 fat pointer
        let ptr_injectable: *const dyn Injectable = Arc::into_raw(entry.obj);
        // SAFETY: fat pointer 位拆解（data + vtable 两段 usize），仅观察用途
        let bits: [usize; 2] = unsafe { std::mem::transmute_copy(&ptr_injectable) };
        // SAFETY: 拼出的 fat pointer 与编译器 upcast 产物位级相同——data 来自
        // `Arc::into_raw`，vtable 是 coercion 生成的真实 dyn Trait vtable，满足
        // `Arc::from_raw` 契约（head 三槽 drop/size/align 与该具体类型一致）。
        let ptr_trait: *const Trait =
            unsafe { std::mem::transmute_copy(&[bits[0], entry.vtable as usize]) };
        Ok(unsafe { Arc::from_raw(ptr_trait) })
    }

    /// 按 trait 获取唯一实现 → `Arc<dyn Trait>`
    ///
    /// 通过 trait 实现索引（`get_impl_component_names_by_trait`）
    /// 获取该 trait 的所有实例名，要求恰好一个。
    ///
    /// 错误：
    /// - 0 个实现类型 → `TraitImplNotFound`
    /// - 多个实现类型或实例 → `AmbiguousTraitImpl`
    pub fn get_component_by_trait<Trait: Injectable + ?Sized>(
    ) -> Result<Arc<Trait>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let trait_name = std::any::type_name::<Trait>().to_string();

        let names = get_impl_component_names_by_trait(trait_type_id);
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
    /// 通过 trait 实现索引 + 仓库扫描（`get_impl_component_names_by_trait`）
    /// 获取该 trait 的所有实例名，逐个从 `TRAIT_OBJ_CACHE` 取出。
    ///
    /// 如果 trait 没有实现类型或所有实例都未被缓存，返回空 Vec。
    pub fn get_components_by_trait<Trait: Injectable + ?Sized>(
    ) -> Result<Vec<Arc<Trait>>, ComponentError> {
        let trait_type_id = TypeId::of::<Trait>();
        let trait_name = std::any::type_name::<Trait>().to_string();

        let mut results = Vec::new();
        for name in get_impl_component_names_by_trait(trait_type_id).iter() {
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
