# 应用核心模块使用说明

本套代码提供了一组轻量级、可组合的基础设施，用于构建结构清晰、配置驱动、组件解耦并支持定时任务的 Rust 应用。
它通过 **三个核心属性宏** 实现自动注册与依赖注入，并配合运行时管理完成完整的应用生命周期控制。

以下内容将从 **使用方式** 与 **实现原理** 两个维度进行说明，并在最后提供一个完整可运行的示例。

---

## 一、核心功能与使用方式

### 1. 自动注册组件：`#[auto_component]`

#### 使用方式

```rust
use simple_starter_macro::auto_component;

// 默认以返回类型名为组件名（如 "Database"）
#[auto_component]
fn create_db() -> Database {
    Database::connect("...")
}

// 或显式指定名称
#[auto_component(name = "main_cache")]
fn build_cache() -> RedisCache {
    RedisCache::new()
}
```

#### 🔧 实现原理

- 宏在编译期 **生成一个无捕获闭包**，调用原函数并将结果封装为 `Arc<T>`。该闭包通过inventory提交到静态注册表，作为组件工厂的构造器。
- 该闭包通过`inventory`提交到静态注册表，作为组件工厂的构造器。
- 由于闭包不捕获任何环境变量，可零成本转为函数指针，与`ComponentFactory::constructor`类型兼容。
- 应用启动时，自动遍历所有工厂并执行构造，存入全局组件仓库 `COMPONENT_REPOSITORY`。

> ⚠ 要求：函数无参、有显式返回类型；不支持泛型顶层返回类型。

---

### 2. 自动生成依赖获取方法：`#[auto_inject]`

#### 使用方式

```rust
use simple_starter_macro::auto_inject;

#[auto_inject(
    types(Logger, Database),           // 获取某类型的所有组件
    names(("auth", AuthService))       // 按名称获取特定组件
)]
struct AppContext {}

// 自动生成：
// - get_loggers() -> Vec<Arc<Logger>>
// - get_databases() -> Vec<Arc<Database>>
// - get_auth() -> Arc<AuthService>
```

#### 🔧 实现原理

- 宏解析 `types` 和 `names` 参数，提取类型与名称。
- 为每项生成对应的 `get_xxx()` 方法，内部调用 `AppCoreUtil` 的组件查询方法。这些方法直接返回 `Arc<T>`，使用者可根据组件 `T`
  本身的线程安全特性（如是否为 `Sync`）来决定如何使用。
- 方法名由驼峰转蛇形（如 `MyService` → `my_service`）。

---

### 3. 注册异步定时任务：`#[cron_job]`

#### 使用方式

```rust
use simple_starter_macro::cron_job;

#[cron_job("0 2 * * *")] // 每天凌晨2点执行
async fn cleanup_task() {
    println!("Running daily cleanup...");
}
```

#### 🔧 实现原理

- 宏验证函数是否为`async fn`且无参数。
- **生成一个无捕获闭包**，内部调用原 async 函数并返回`Pin<Box<dyn Future<Output = ()> + Send>>`。
- 闭包无状态，可直接作为函数指针使用，**无需生成额外具名函数**，避免命名冲突和符号表污染。
- 通过`inventory`提交`CronJob { name, cron_expr, runner: closure }`。
- 启动时，使用`tokio-cron-scheduler`注册所有任务。

---

### 4. 插件机制（可选）

#### 使用方式

```rust
use serde::Deserialize;
use simple_starter_core::{anyhow::Result, AppCoreUtil, Plugin};
use toml::Value;

#[derive(Debug, Deserialize)]
struct MyConfig {
    enabled: bool,
}

struct MyPlugin;

impl Plugin for MyPlugin {
    fn name(&self) -> &'static str { "MyPlugin" }

    fn dependencies(&self) -> &[&'static str] { &["OtherPlugin"] }

    fn default_config(&self) -> Value {
        let default_config = toml::toml! { 
            [my_plugin] 
            enabled = true 
        };
        Value::Table(default_config)
    }

    fn init(&mut self) -> Result<()> {
        let cfg: MyConfig = AppCoreUtil::get_config_to_struct("my_plugin")?;
        // 初始化逻辑
        Ok(())
    }

    fn shutdown_hook(&mut self) -> Option<Box<dyn FnOnce()>> {
        Some(Box::new(|| println!("Shutting down MyPlugin")))
    }
}
```

#### 🔧 实现原理

- 插件通过 `Application::register_plugin()` 注册。
- 启动时按依赖关系进行拓扑排序，检测循环或缺失依赖。
- 按顺序调用 `init()`，退出时逆序调用 `shutdown_hook()`。

---

### 5. 配置管理

#### 配置文件位置

- 默认：`./resources/application.toml`
- 支持 profile：`application-{profile}.toml`（profile 来自 `APP_PROFILE` 环境变量或 `app.profile` 配置）

#### 读取配置

```rust
// 按路径读取原始值
let level = AppCoreUtil::get_config_value_by_path("app.log_level") ?;

// 反序列化为结构体
#[derive(Deserialize)]
struct DbConfig {
    url: String
}
let db: DbConfig = AppCoreUtil::get_config_to_struct("database") ?;
```

#### 合并策略

插件默认配置 → `application.toml` → `application-{profile}.toml`（后者覆盖前者）

---

### 6. 应用主入口

```rust
use simple_starter_core::Application;

fn main() {
    Application::new()
        .register_plugin(MyPlugin)
        .add_startup_hook(|| println!("Application started!"))
        .add_shutdown_hook(|| info!("Performing final cleanup before plugin shutdown..."))
        .run(); // 阻塞直到退出
}
```

#### 启动流程

##### 1. **配置阶段**：

- 收集所有插件的默认配置
- 加载 `application.toml` 并合并（含 profile 覆盖）

##### 2. **初始化阶段**：

- 初始化 `AppBasicPlugin`（日志、配置等核心能力）
- 自动注册 `#[auto_component]` 标记的全局组件
- 对插件按依赖关系进行拓扑排序
- 依次初始化所有插件（按依赖顺序）
- 执行 `startup_hooks`

##### 3. **运行阶段**（二选一）：

- **默认模式**（无 `main_loop_hook`）：
  在当前线程启动 Tokio Runtime，监听退出信号，适合服务端/CLI。
- **自定义主循环模式**（有 `main_loop_hook`）：
  将 `Application` 所有权移交钩子函数，由用户控制主循环，适合 GUI。

> 若存在 `CronJob`，会自动启动调度器（后台线程或当前 Runtime）。

##### 4. **关闭阶段**：

- 触发退出信号（Ctrl+C / SIGTERM 或用户主动结束）
- 执行 `shutdown_hooks`
- 逆序调用插件的 `shutdown_hook`
- 清理全局组件仓库

> ⚠ 注意：若配置加载或插件初始化失败，程序将直接 `exit(1)`。

---

## 二、完整可运行示例

### 项目结构

```
your-app/
├── Cargo.toml
├── src/main.rs
└── resources
    ├── application.toml
    └── application-dev.toml
```

### `Cargo.toml`

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2024"

[dependencies]
simple-starter-core = { path = "../simple-starter/simple-starter-core" }
simple-starter-macro = { path = "../simple-starter/simple-starter-macro" }
serde = { version = "1.0.228", features = ["derive"] }
toml = "0.9.8"
```

### `resources/application.toml`

```toml
[app]
log_level = "INFO"
profile = "dev"

[database]
url = "sqlite://data.db"
```

### `resources/application-dev.toml`

```toml
[app]
log_level = "DEBUG"

[database]
url = "postgres://localhost:5432/myapp_dev"
```

### `src/main.rs`

```rust
use serde::Deserialize;
use simple_starter_core::{anyhow::Result, AppCoreUtil, Application, Plugin};
use simple_starter_core::tracing::info;
use simple_starter_macro::{auto_component, auto_inject, cron_job};
use toml::{Value};

// --- 配置结构 ---
#[derive(Deserialize)]
struct DbConfig {
    url: String,
}

// --- 组件定义 ---
struct Database {
    url: String,
}

impl Database {
    fn connect(url: &str) -> Self {
        info!("Connecting to DB: {}", url);
        Self { url: url.to_string() }
    }
}

// --- 自动注册组件 ---
#[auto_component]
fn database_factory() -> Database {
    let cfg: DbConfig = AppCoreUtil::get_config_to_struct("database")
        .expect("Failed to load database config");
    Database::connect(&cfg.url)
}

// --- 上下文注入 ---
#[auto_inject(types(Database))]
struct AppContext {}

// --- 定时任务 ---
#[cron_job("*/10 * * * * *")] // 每10秒执行一次
async fn heartbeat() {
    info!("Heartbeat tick!");
}

// --- 插件（可选）---
struct MyAppPlugin;

impl Plugin for MyAppPlugin {
    fn name(&self) -> &'static str { "MyAppPlugin" }

    fn default_config(&self) -> Value {
        let table = toml::toml! {
            [database]
            url = "mysql://user:password@localhost:3306/my_db"
        };
        Value::Table(table)
    }

    fn init(&mut self) -> Result<()> {
        // 使用注入上下文
        let ctx = AppContext {};
        let db = ctx.get_databases().first().unwrap().clone();
        info!("Plugin initialized with DB URL: {}", db.url);
        Ok(())
    }

    fn shutdown_hook(&mut self) -> Option<Box<dyn FnOnce()>> {
        Some(Box::new(|| info!("Shutting down MyAppPlugin")))
    }
}

// --- 主函数 ---
fn main() {
    Application::new()
        .register_plugin(MyAppPlugin)
        .add_startup_hook(|| info!("Application is running!"))
        .add_shutdown_hook(|| info!("Performing final cleanup before plugin shutdown..."))
        .run(); // 阻塞直到退出
}
```

---

## 三、总结

通过 `#[auto_component]`、`#[auto_inject]`、`#[cron_job]` 三大属性宏，配合 `Application` 主控和 `AppCoreUtil` 工具类，你可以：

- **自动注册**任意构造函数为全局组件；
- **自动生成**类型安全的依赖获取方法；
- **声明式定义**异步定时任务；
- **集中管理**配置与插件生命周期。

整套机制完全基于 **Rust** 编译期能力（`inventory` + 过程宏），无运行时反射，类型安全。组件以 `Arc<T>` 形式存储，其并发控制策略由组件
`T` 自身的设计决定（例如，若 `T` 内部包含 `Mutex` 或 `RwLock`，则可实现内部可变性）。适合构建中小型服务或后台应用。