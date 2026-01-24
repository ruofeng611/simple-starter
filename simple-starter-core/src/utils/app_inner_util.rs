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
