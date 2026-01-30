use crate::core::app_component::{ComponentProcessor, ComponentProcessorFactory};
use crate::core::app_error::ComponentError;
use crate::core::app_types::ComponentKey;
use crate::global_state::COMPONENT_REPOSITORY;
use crate::utils::app_inner_util::find_cycle_path;
use anyhow::{Context, anyhow};
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

        // 5. 收集依赖信息
        dependency_map.insert(name.to_string(), (key, Vec::from(factory.dependencies)));
    }

    // 6. 计算启动顺序 (拓扑排序)
    let sorted_keys = resolve_component_order(&dependency_map)?;

    // 7. 保存顺序到全局变量
    {
        let mut order_guard = COMPONENT_ORDER
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock COMPONENT_ORDER (Poisoned)"))?;
        *order_guard = sorted_keys;
    }

    Ok(())
}

/// 解析组件启动顺序 (Kahn 拓扑排序算法)
fn resolve_component_order(
    dep_map: &HashMap<String, (ComponentKey, Vec<&'static str>)>,
) -> anyhow::Result<Vec<ComponentKey>> {
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    // 1. 初始化图节点
    for name in dep_map.keys() {
        in_degree.insert(name.clone(), 0);
        graph.entry(name.clone()).or_default();
    }

    // 2. 构建图连边和入度表
    for (name, (_, deps)) in dep_map {
        for dep_name in deps {
            // 校验依赖是否存在
            if !dep_map.contains_key(*dep_name) {
                return Err(anyhow!(
                    "Component '{}' depends on '{}', but '{}' is not registered.",
                    name,
                    dep_name,
                    dep_name
                ));
            }
            // 依赖 -> 依赖者 (A 依赖 B，则 B 先启动，边为 B -> A)
            graph
                .entry(dep_name.to_string())
                .or_default()
                .push(name.clone());
            *in_degree.get_mut(name).unwrap() += 1;
        }
    }

    // 3. 将所有入度为 0 的节点加入队列
    let mut queue: VecDeque<String> = VecDeque::new();
    for (name, &degree) in &in_degree {
        if degree == 0 {
            queue.push_back(name.clone());
        }
    }

    let mut sorted_keys = Vec::new();

    // 4. 处理队列
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

    // 5. 循环依赖检测
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
    for key in &sorted_keys {
        let mut entry = COMPONENT_REPOSITORY.get_mut(key).unwrap();

        let processor = entry.value_mut();
        processor
            .create()
            .await
            .with_context(|| format!("Failed to create component: {}", key.1))?;

        tracing::debug!("Component created: {}", key.1);
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

    // 2. 逆序遍历
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
