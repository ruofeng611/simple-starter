//! Web 服务配置结构定义。
//!
//! 从 TOML 配置文件中解析 `web` 节点，控制服务器绑定、线程、日志等行为。

use serde::{Deserialize, Serialize};

/// Web 服务运行时配置。
///
/// 支持从应用配置中加载如下字段：
/// - `port`: 监听端口（默认 8080）
/// - `binding`: 绑定地址（如 "0.0.0.0"）
/// - `base_path`: 可选的全局路径前缀（如 "/api"）
/// - `worker_thread_num`: Tokio 运行时工作线程数（可选，必须 >0）
/// - `worker_thread_name`: 工作线程命名模板
/// - `log_include_headers`: 是否在 trace 日志中包含 HTTP 头
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct WebConfig {
    pub port: u16,
    pub binding: String,
    pub base_path: Option<String>,
    pub worker_thread_num: Option<u8>,      // 注意：u8 是为了防止过大值，实际转为 usize
    pub worker_thread_name: String,
    pub log_include_headers: bool,
}