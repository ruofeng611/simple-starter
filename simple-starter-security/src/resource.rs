//! 安全资源定义。
//!
//! 提供 `ResourceEntry` 结构体，用于在编译期通过 `inventory` 静态收集
//! 所有被 `#[security]` 宏标记的 Web 资源信息。

/// 资源条目。
///
/// 由 `#[security]` 宏在编译期生成并注册，包含一个 Web 接口的完整权限元数据。
#[derive(Debug, Clone)]
pub struct ResourceEntry {
    /// 完整路径模式（如 `/api/users/:id`）
    pub path_pattern: &'static str,
    /// 资源标识（权限校验的最小粒度）
    pub resource_id: &'static str,
    /// 资源名称（用于展示）
    pub resource_name: &'static str,
    /// 模块标识
    pub module_id: &'static str,
    /// 模块名称
    pub module_name: &'static str,
}

inventory::collect!(ResourceEntry);
