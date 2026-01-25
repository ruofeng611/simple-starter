//! Web 服务配置结构定义。
//!
//! 从 TOML 配置文件中解析 `web` 节点，控制服务器绑定、线程、日志等行为。

use serde::Deserialize;

/// Web 服务运行时配置。
///
/// 对应配置文件中的 `[web]` 节点。
#[derive(Deserialize, Debug)]
pub(crate) struct WebConfig {
    /// 监听端口（例如 8080）
    pub port: u16,
    /// 绑定地址（例如 "0.0.0.0" 或 "127.0.0.1"）
    pub binding: String,
    /// 全局 API 路径前缀（例如 "/api"）。
    /// 如果设置，所有路由都会挂载到此路径下。
    pub base_path: Option<String>,
    /// HTTP 日志配置：是否在 Trace 日志中记录请求/响应头。
    pub log_include_headers: bool,
}
