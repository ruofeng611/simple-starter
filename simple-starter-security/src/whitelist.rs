//! 白名单定义与匹配逻辑。

/// 白名单条目。
///
/// 格式规则：
/// - `path` 如 `/login` 表示精确匹配
/// - `path` 如 `/login/*` 表示前缀匹配
/// - `method` 为 `Some("GET")` 时只匹配该 HTTP 方法；为 `None` 时匹配所有方法
#[derive(Debug, Clone)]
pub struct WhitelistEntry {
    pub method: Option<String>,
    pub path_pattern: String,
}

impl WhitelistEntry {
    /// 创建一个新的白名单条目。
    pub fn new(method: Option<&str>, path_pattern: impl Into<String>) -> Self {
        Self {
            method: method.map(|s| s.to_uppercase()),
            path_pattern: path_pattern.into(),
        }
    }

    /// 判断请求是否命中当前白名单规则。
    pub fn matches(&self, method: &str, path: &str) -> bool {
        // 方法匹配
        if let Some(ref m) = self.method {
            if m != method.to_uppercase().as_str() {
                return false;
            }
        }

        // 路径匹配
        if self.path_pattern.ends_with("/*") {
            let prefix = &self.path_pattern[..self.path_pattern.len() - 2];
            path.starts_with(prefix)
        } else {
            self.path_pattern == path
        }
    }
}

/// 白名单集合。
#[derive(Debug, Clone, Default)]
pub struct Whitelist {
    entries: Vec<WhitelistEntry>,
}

impl Whitelist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, method: Option<&str>, path_pattern: impl Into<String>) {
        self.entries.push(WhitelistEntry::new(method, path_pattern));
    }

    pub fn is_allowed(&self, method: &str, path: &str) -> bool {
        self.entries.iter().any(|e| e.matches(method, path))
    }
}
