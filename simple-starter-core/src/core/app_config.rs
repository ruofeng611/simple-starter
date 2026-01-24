use serde::Deserialize;

/// 应用基础配置
///
/// 对应 TOML 中的 `[app]` 节
#[derive(Deserialize, Debug)]
pub(crate) struct AppConfig {
    /// 激活的环境配置 (例如: "dev", "prod")，对应加载 application-{profile}.toml
    pub profile: Option<String>,
    /// 应用名称，用于日志显示等
    pub name: Option<String>,
}

/// 日志配置
///
/// 对应 TOML 中的 `[logger]` 节
#[derive(Deserialize, Debug)]
pub(crate) struct LoggerConfig {
    /// 日志级别 (TRACE, DEBUG, INFO, WARN, ERROR)
    pub level: String,
    /// 是否在日志中显示线程 ID
    pub with_thread_id: bool,
    /// 是否在日志中显示线程名称
    pub with_thread_name: bool,
    /// 日志文件保存路径，若为 None 则不写入文件
    pub save_file: Option<String>,
    /// 是否开启控制台输出
    pub enable_console: bool,
    /// 文件写入模式：true=追加，false=覆盖
    pub content_append: bool,
}

/// 运行时配置
///
/// 对应 TOML 中的 `[runtime]` 节
#[derive(Deserialize, Debug)]
pub(crate) struct RuntimeConfig {
    /// 工作线程数量。None=默认(CPU核数), 1=单线程运行时, >1=多线程运行时
    pub worker_thread_num: Option<u8>,
    /// 工作线程名称
    pub worker_thread_name: String,
}
