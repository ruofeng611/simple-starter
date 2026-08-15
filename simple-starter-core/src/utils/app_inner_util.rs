use crate::core::app_component::{ComponentProcessor, TraitImplRegistration};
use crate::global_state::{COMPONENT_REPOSITORY, TRAIT_INSTANCE_NAMES, TYPE_INSTANCE_NAMES};
use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::hash::Hash;
use toml::Value;


/// 深度合并 TOML 值
///
/// 将 `overlay` 中的配置覆盖到 `base` 上。如果两者都是 Table，则递归合并。
pub(crate) fn merge_toml_values(mut base: Value, overlay: Value) -> Value {
    match (base.as_table_mut(), overlay.as_table()) {
        (Some(base_map), Some(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                match base_map.get(key) {
                    Some(base_val) => {
                        let merged = merge_toml_values(base_val.clone(), overlay_val.clone());
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

/// 构建 trait 实现索引：trait_type_id → 所有实现的具体类型
///
/// 遍历 inventory 全量 `TraitImplRegistration`，按 trait 分组取实现类型。
/// 供组件加载（DFS trait 依赖展开）与条件注册（ConditionContext 快照）共用。
pub(crate) fn build_trait_impl_index() -> HashMap<TypeId, Vec<TypeId>> {
    let mut index: HashMap<TypeId, Vec<TypeId>> = HashMap::new();
    for reg in inventory::iter::<TraitImplRegistration> {
        index
            .entry(reg.trait_type_id)
            .or_default()
            .push(reg.impl_type_id);
    }
    index
}

/// 构建实现类型注册索引：impl_type_id → 匹配的 trait 实现注册列表（一对多）
///
/// 遍历 inventory 全量 `TraitImplRegistration`，按实现类型分组保留注册项
/// （含 accessor）。供 `populate_trait_obj_cache` 按组件具体类型直接查询，
/// 替代每次对全量注册的线性扫描。一个组件类型可注册多个 trait 实现，故值为列表。
pub(crate) fn build_impl_registration_index(
) -> HashMap<TypeId, Vec<&'static TraitImplRegistration>> {
    let mut index: HashMap<TypeId, Vec<&'static TraitImplRegistration>> = HashMap::new();
    for reg in inventory::iter::<TraitImplRegistration> {
        index.entry(reg.impl_type_id).or_default().push(reg);
    }
    index
}

/// 扫描当前仓库构建 组件名快照 + 具体类型索引
///
/// 组件类型取自处理器本身（`ComponentProcessor::type_id`），而非仓库 key。
/// 覆盖全部已注册组件。预计算快照，DFS 展开依赖时直接查询，
/// 避免依赖展开路径上重复扫描仓库（快照语义与注册期一致）。
/// 供组件加载（DFS 索引，过滤后调用）与条件注册（ConditionContext 快照，过滤前调用）共用。
pub(crate) fn build_component_indexes() -> (HashSet<String>, HashMap<TypeId, Vec<String>>) {
    let mut names = HashSet::new();
    let mut type_index: HashMap<TypeId, Vec<String>> = HashMap::new();
    for entry in COMPONENT_REPOSITORY.iter() {
        let name = entry.key().clone();
        // 完全限定语法：ComponentProcessor: Any，需消除 Any::type_id 歧义；显式解引用 Box
        let type_id = ComponentProcessor::type_id(&**entry.value());
        names.insert(name.clone());
        type_index.entry(type_id).or_default().push(name);
    }
    (names, type_index)
}

/// 获取具体类型下的所有组件实例名
///
/// 从 `TYPE_INSTANCE_NAMES` 索引读取（每个组件 create 后由
/// `populate_trait_obj_cache` 填充）。
/// 供 `get_component` 兜底查找（组件自定义名称时）使用。
pub(crate) fn get_component_names_by_type(type_id: TypeId) -> Vec<String> {
    TYPE_INSTANCE_NAMES
        .get(&type_id)
        .map(|entry| entry.value().clone())
        .unwrap_or_default()
}

/// 获取 trait 下所有具体实现组件的实例名
///
/// 从 `TRAIT_INSTANCE_NAMES` 索引读取（`populate_trait_obj_cache` 时同步填充）。
/// DFS 保证依赖组件先于依赖者创建，依赖者 create 阶段访问时索引必已填充。
pub(crate) fn get_impl_component_names_by_trait(trait_type_id: TypeId) -> Vec<String> {
    TRAIT_INSTANCE_NAMES
        .get(&trait_type_id)
        .map(|entry| entry.value().clone())
        .unwrap_or_default()
}

/// 获取类型的简短名称（去掉命名空间）
pub(crate) fn get_short_type_name<T: ?Sized + 'static>() -> String {
    let full_name = std::any::type_name::<T>();
    full_name
        .split("::")
        .last()
        .unwrap_or(full_name)
        .to_string()
}

/// 寻找图中的循环依赖路径
///
/// 用于在拓扑排序失败时，生成易读的错误信息 (例如: A -> B -> A)。
pub(crate) fn find_cycle_path<T>(
    graph: &HashMap<T, Vec<T>>,
    in_degree: &HashMap<T, usize>,
) -> String
where
    T: Eq + Hash + Clone + Display,
{
    // 1. 找出所有入度不为 0 的节点（即环中的节点或依赖环的节点）
    let remaining_nodes: Vec<T> = in_degree
        .iter()
        .filter(|(_, deg)| **deg > 0)
        .map(|(k, _)| k.clone())
        .collect();

    // 2. DFS 搜索环
    if let Some(start_node) = remaining_nodes.first() {
        let mut visited = HashSet::new();
        let mut path = Vec::new();

        if let Some(cycle) = dfs_find_cycle(start_node, graph, in_degree, &mut visited, &mut path) {
            return cycle
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join(" -> ");
        }
    }

    format!(
        "Complex cycle detected: {:?}",
        remaining_nodes
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
    )
}

/// DFS 递归辅助函数
fn dfs_find_cycle<T>(
    curr: &T,
    graph: &HashMap<T, Vec<T>>,
    in_degree: &HashMap<T, usize>,
    visited: &mut HashSet<T>,
    path: &mut Vec<T>,
) -> Option<Vec<T>>
where
    T: Eq + Hash + Clone,
{
    visited.insert(curr.clone());
    path.push(curr.clone());

    if let Some(neighbors) = graph.get(curr) {
        for neighbor in neighbors {
            if *in_degree.get(neighbor).unwrap_or(&0) > 0 {
                if visited.contains(neighbor) {
                    // 发现环
                    if let Some(pos) = path.iter().position(|x| x == neighbor) {
                        let mut cycle = path[pos..].to_vec();
                        cycle.push(neighbor.clone());
                        return Some(cycle);
                    }
                } else {
                    if let Some(cycle) = dfs_find_cycle(neighbor, graph, in_degree, visited, path) {
                        return Some(cycle);
                    }
                }
            }
        }
    }

    // 回溯
    path.pop();
    visited.remove(curr);
    None
}
