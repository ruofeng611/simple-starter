//! # 组件自动加载器
//!
//! 在应用启动时，自动收集所有通过 `#[auto_component]` 注册的 `ComponentFactory`，
//! 并调用其构造函数将组件实例注册到全局仓库。

use crate::ComponentFactory;
use crate::global_state::{COMPONENT_REPOSITORY, ComponentKey};
use anyhow::{Result, anyhow};
use dashmap::Entry;
use tracing::debug;

/// 自动加载并注册所有组件工厂创建的组件
///
/// 遍历 `inventory` 中所有 `ComponentFactory`，调用其 `constructor`，
/// 并将结果存入 `COMPONENT_REPOSITORY`。
pub(crate) fn auto_collect_global_component_load() -> Result<()> {
    for component_factory in inventory::iter::<ComponentFactory> {
        let component_key: ComponentKey = (
            component_factory.type_id,
            component_factory.name.to_string(),
        );
        let component = (component_factory.constructor)();

        match COMPONENT_REPOSITORY.entry(component_key) {
            Entry::Occupied(_) => {
                return Err(anyhow!(
                    "Component with type {:?} and name '{}' is already registered",
                    component_factory.type_id,
                    component_factory.name
                ));
            }
            Entry::Vacant(vacant) => {
                vacant.insert(component);
            }
        }

        debug!("Component '{}' initialized...", component_factory.name);
    }

    Ok(())
}
