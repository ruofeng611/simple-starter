use crate::core::app_component::{ComponentProcessor, ComponentProcessorFactory, TraitImplRegistration};
use crate::core::app_error::ComponentError;
use crate::core::app_types::{ComponentKey};
use crate::global_state::{COMPONENT_REPOSITORY, INSTANCE_NAMES_BY_TRAIT, TRAIT_OBJ_CACHE};
use crate::utils::app_inner_util::{find_cycle_path, find_instance_names_by_type};
use anyhow::{Context, anyhow};
use std::any::TypeId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

/// 全局组件启动顺序队列
///
/// 用于记录组件的启动顺序，以便在销毁时按逆序进行。
/// 受 Mutex 保护，并发读写。
pub(crate) static COMPONENT_ORDER: Mutex<Vec<ComponentKey>> = Mutex::new(Vec::new());

/// 加载并初始化所有组件
///
/// 包含三个步骤：
/// 1. `register_components`: 扫描、注册并计算拓扑顺序。
/// 2. `run_creation_and_init`: 执行组件的 create 和 init 方法。
pub(crate) async fn component_repository_load() -> anyhow::Result<()> {
    register_components()?;
    run_creation_and_init().await?;
    Ok(())
}

/// 注册组件阶段
///
/// 遍历 inventory 收集的工厂，校验名称唯一性，计算依赖关系，并存入仓库。
fn register_components() -> anyhow::Result<()> {
    let mut dependency_map: HashMap<String, (ComponentKey, Vec<&'static str>)> = HashMap::new();
    let mut trait_dep_map: HashMap<String, Vec<TypeId>> = HashMap::new();
    let mut registered_names: HashSet<String> = HashSet::new();

    // 1. 遍历所有注册的工厂
    for factory in inventory::iter::<ComponentProcessorFactory> {
        let name = factory.name;
        let type_id = factory.type_id;

        // 2. 严格的名称唯一性检查
        if registered_names.contains(name) {
            return Err(anyhow::anyhow!(
                "Duplicate component name detected: '{}'. Component names must be globally unique.",
                name
            ));
        }
        registered_names.insert(name.to_string());

        // 3. 构造组件 Wrapper (此时 inner 为 None)
        let processor: Box<dyn ComponentProcessor> = (factory.constructor)();
        let key: ComponentKey = (type_id, name.to_string());

        // 4. 双重防御：检查仓库中是否已存在
        if COMPONENT_REPOSITORY.contains_key(&key) {
            return Err(ComponentError::AlreadyExists {
                type_name: "Unknown(TypeID Check)".to_string(),
                name: name.to_string(),
            }
            .into());
        }

        COMPONENT_REPOSITORY.insert(key.clone(), processor);

        // 5. 收集依赖信息（具体类型依赖 + trait 依赖）
        dependency_map.insert(name.to_string(), (key, Vec::from(factory.dependencies)));

        // 收集 trait 依赖：调用每个函数指针获取 TypeId
        let trait_type_ids: Vec<TypeId> = factory
            .trait_dependencies
            .iter()
            .map(|get_type_id| get_type_id())
            .collect();
        if !trait_type_ids.is_empty() {
            trait_dep_map.insert(name.to_string(), trait_type_ids);
        }
    }

    // 6. 构建 trait 实现索引（局部变量，启动后不再需要）
    let trait_impl_index: HashMap<TypeId, Vec<&TraitImplRegistration>> = build_trait_impl_index();

    // 7. 计算启动顺序 (拓扑排序，含 trait 依赖解析)
    let sorted_keys = resolve_component_order(&dependency_map, &trait_dep_map, &trait_impl_index)?;

    // 8. 追加顺序到全局变量
    {
        let mut order_guard = COMPONENT_ORDER
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock COMPONENT_ORDER (Poisoned)"))?;
        order_guard.extend(sorted_keys);
    }

    Ok(())
}

/// 构建 trait 实现索引（仅在 `register_components` 期间使用）
///
/// 从 inventory 中读取所有 `TraitImplRegistration`，
/// 按 `trait_type_id` 分组存入局部 `HashMap`。
fn build_trait_impl_index() -> HashMap<TypeId, Vec<&'static TraitImplRegistration>> {
    let mut index: HashMap<TypeId, Vec<&'static TraitImplRegistration>> = HashMap::new();
    for reg in inventory::iter::<TraitImplRegistration> {
        index
            .entry(reg.trait_type_id)
            .or_default()
            .push(reg);
    }
    index
}

/// 解析组件启动顺序 (Kahn 拓扑排序算法)
///
/// - `dependency_map`: 组件名 → (ComponentKey, 具体类型依赖列表)
/// - `trait_dep_map`: 组件名 → 依赖的 trait TypeId 列表
/// - `trait_impl_index`: trait_type_id → 所有实现元数据（由 `build_trait_impl_index` 构建）
///
/// trait 依赖通过 `trait_dep_map` 中的 TypeId 直接查找 `trait_impl_index`，
/// 展开为所有具体实现的实例名，确保每个实现组件都在依赖者之前创建。
fn resolve_component_order(
    dep_map: &HashMap<String, (ComponentKey, Vec<&'static str>)>,
    trait_dep_map: &HashMap<String, Vec<TypeId>>,
    trait_impl_index: &HashMap<TypeId, Vec<&'static TraitImplRegistration>>,
) -> anyhow::Result<Vec<ComponentKey>> {
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    // 1. 初始化图节点
    for name in dep_map.keys() {
        in_degree.insert(name.clone(), 0);
        graph.entry(name.clone()).or_default();
    }

    // 2. 构建图连边和入度表 —— 具体类型依赖
    for (name, (_, deps)) in dep_map {
        for dep_name in deps {
            if !dep_map.contains_key(*dep_name) {
                return Err(anyhow!(
                    "Component '{}' depends on '{}', but '{}' is not registered.",
                    name,
                    dep_name,
                    dep_name
                ));
            }
            graph
                .entry(dep_name.to_string())
                .or_default()
                .push(name.clone());
            *in_degree.get_mut(name).unwrap() += 1;
        }
    }

    // 3. 构建图连边 —— trait 依赖（通过 TypeId 直接查找 trait_impl_index）
    for (name, trait_type_ids) in trait_dep_map {
        for &type_id in trait_type_ids {
            let impls = trait_impl_index
                .get(&type_id)
                .ok_or_else(|| {
                    anyhow!(
                        "Component '{}' depends on a trait (TypeId={:?}) that has no registered implementations",
                        name,
                        type_id
                    )
                })?;

            let mut resolved_any = false;
            for reg in impls.iter() {
                let instance_names = find_instance_names_by_type(reg.impl_type_id);
                for impl_name in &instance_names {
                    if !dep_map.contains_key(impl_name.as_str()) {
                        return Err(anyhow!(
                            "Component '{}' depends on a trait (TypeId={:?}), but its implementation '{}' is not registered.",
                            name,
                            type_id,
                            impl_name
                        ));
                    }
                    graph
                        .entry(impl_name.clone())
                        .or_default()
                        .push(name.clone());
                    *in_degree.get_mut(name).unwrap() += 1;
                    resolved_any = true;
                }
            }

            if !resolved_any {
                return Err(anyhow!(
                    "Component '{}' depends on a trait (TypeId={:?}) that has no registered component instances",
                    name,
                    type_id
                ));
            }
        }
    }

    // 4. 将所有入度为 0 的节点加入队列
    let mut queue: VecDeque<String> = VecDeque::new();
    for (name, &degree) in &in_degree {
        if degree == 0 {
            queue.push_back(name.clone());
        }
    }

    let mut sorted_keys = Vec::new();

    // 5. 处理队列
    while let Some(name) = queue.pop_front() {
        if let Some((key, _)) = dep_map.get(&name) {
            sorted_keys.push(key.clone());
        }

        if let Some(neighbors) = graph.get(&name) {
            for neighbor in neighbors {
                if let Some(degree) = in_degree.get_mut(neighbor) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }
    }

    // 6. 循环依赖检测
    if sorted_keys.len() != dep_map.len() {
        let cycle_path = find_cycle_path(&graph, &in_degree);
        return Err(anyhow!(
            "Cyclic dependency detected in components: {}",
            cycle_path
        ));
    }

    Ok(sorted_keys)
}

/// 运行组件创建和初始化
async fn run_creation_and_init() -> anyhow::Result<()> {
    // 1. 获取排序好的 Key 列表 (Clone 出来，避免持有锁进行 await)
    let sorted_keys: Vec<ComponentKey> = {
        let guard = COMPONENT_ORDER
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock COMPONENT_ORDER (Poisoned)"))?;
        guard.clone()
    };

    // 2. 阶段一：创建 (Create)
    //    创建完成后立即填充 trait object 缓存，确保后续组件在 create 阶段即可
    //    通过 get_component_by_trait 获取依赖。拓扑排序保证被依赖的组件先创建。
    for key in &sorted_keys {
        {
            let mut entry = COMPONENT_REPOSITORY.get_mut(key).unwrap();
            let processor = entry.value_mut();
            processor
                .create()
                .await
                .with_context(|| format!("Failed to create component: {}", key.1))?;
        } // Ref Mut 在此 drop，避免与 populate_trait_obj_cache 中的 get() 冲突

        tracing::debug!("Component created: {}", key.1);

        // 创建后立即缓存该组件的 trait object，使后续组件在 create 时可获取
        populate_trait_obj_cache(key)?;
    }

    // 3. 阶段二：初始化 (Init)
    for key in &sorted_keys {
        let mut entry = COMPONENT_REPOSITORY.get_mut(key).unwrap();

        let processor = entry.value_mut();
        processor
            .init()
            .await
            .with_context(|| format!("Failed to init component: {}", key.1))?;

        tracing::debug!("Component initialized: {}", key.1);
    }

    Ok(())
}

/// 为指定组件填充 trait object 缓存
///
/// 遍历所有 `TraitImplRegistration`，找到匹配当前组件具体类型的注册项，
/// 通过 accessor 将 `Arc<ConcreteType>` 转换为 `Arc<dyn Injectable>`，
/// 以 **(trait_type_id, 组件实例名)** 为 key 存入 `TRAIT_OBJ_CACHE`。
///
/// 使用组件实例名作为 cache key（而非 TraitImplRegistration 名称），
/// 确保同一具体类型的多个实例（如通过 provider 创建的同类型不同名称组件）
/// 各自拥有独立的 cache 条目。
fn populate_trait_obj_cache(key: &ComponentKey) -> anyhow::Result<()> {
    let entry = COMPONENT_REPOSITORY.get(key).ok_or_else(|| {
        anyhow::anyhow!("Component '{}' not found in repository", key.1)
    })?;
    let processor = entry.value();

    // 获取类型擦除的 Arc
    let arc_any = match processor.get_inner_arc_any() {
        Some(a) => a,
        None => return Err(anyhow::anyhow!("Component '{}' not created", key.1)), // 还未 create，不可能发生，直接报错
    };

    let component_instance_name = &key.1;

    // 查找匹配的 trait 实现注册
    for reg in inventory::iter::<TraitImplRegistration> {
        if reg.impl_type_id == key.0 {
            if let Some(trait_obj) = (reg.accessor)(arc_any.clone()) {
                // cache key: (trait_type_id, 组件实例名)
                let cache_key = (reg.trait_type_id, component_instance_name.clone());
                TRAIT_OBJ_CACHE.insert(cache_key, trait_obj);
                {
                    let mut names_entry = INSTANCE_NAMES_BY_TRAIT
                        .entry(reg.trait_type_id)
                        .or_default();
                    if !names_entry.contains(component_instance_name) {
                        names_entry.push(component_instance_name.clone());
                    }
                }
                tracing::debug!(
                    "Registered trait object: instance '{}' as trait TypeId={:?}",
                    component_instance_name,
                    reg.trait_type_id
                );
            }
        }
    }
    Ok(())
}

/// 关闭并销毁所有组件
///
/// 按照启动顺序的**逆序**进行销毁。
pub(crate) async fn shutdown_components() -> anyhow::Result<()> {
    // 1. 获取并清空全局顺序列表 (防止多次 shutdown 重复执行)
    let sorted_keys: Vec<ComponentKey> = {
        let mut guard = COMPONENT_ORDER
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock COMPONENT_ORDER (Poisoned)"))?;
        std::mem::take(&mut *guard)
    };

    if sorted_keys.is_empty() {
        tracing::warn!("No components to shutdown or COMPONENT_ORDER is already empty.");
        return Ok(());
    }

    //2. 销毁前先清空 trait object 缓存，释放对组件实例的额外引用，
    //   确保后续 destroy() 中 Arc::try_unwrap 的 refcount 为 1。
    TRAIT_OBJ_CACHE.clear();
    INSTANCE_NAMES_BY_TRAIT.clear();

    // 3. 逆序遍历
    for key in sorted_keys.iter().rev() {
        // 3. 从仓库中移除所有权
        if let Some((_, mut processor)) = COMPONENT_REPOSITORY.remove(key) {
            // 4. 调用 destroy
            if let Err(e) = processor.destroy().await {
                // 销毁失败只记录错误，不中断流程，保证其他组件有机会销毁
                tracing::error!("Error destroying component '{}': {:?}", key.1, e);
            } else {
                tracing::debug!("Component '{}' destroyed successfully.", key.1);
            }
        }
    }

    Ok(())
}
