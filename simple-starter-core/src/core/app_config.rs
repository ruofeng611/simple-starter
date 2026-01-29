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

    /// 是否开启控制台输出
    pub enable_console: bool,

    /// 是否在日志中显示线程 ID
    pub with_thread_id: bool,
    /// 是否在日志中显示线程名称
    pub with_thread_name: bool,
    /// 日志时区，默认使用UTC时间
    pub timezone: Option<String>,

    /// 日志文件路径
    pub log_dir: Option<String>,
    /// 日志文件名
    pub file_name: String,
    /// 日志文件最大数量
    pub max_file_number: usize,
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
