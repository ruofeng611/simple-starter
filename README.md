这是一个名为 **simple-starter** 的 Rust 应用程序启动库说明文档。它旨在通过宏和依赖注入机制，简化复杂 Rust 应用的构建、生命周期管理和模块解耦。

---

# simple-starter

**simple-starter** 是一个模块化的 Rust 应用程序启动库。它提供了一套基于 **宏（Macros）** 和 **依赖注入（DI）** 的机制，用于自动管理组件生命周期、依赖关系、配置加载以及 Web 路由注册，让你专注于业务逻辑的实现。

## ✨ 主要特性

*   **自动依赖注入**：通过 `#[component]` 和 `#[inject]` 宏实现组件的自动注册与装配，支持按类型或名称注入。
*   **智能生命周期管理**：自动计算组件依赖拓扑结构，按正确顺序执行 `create` -> `init`，并在退出时逆序 `destroy`。
*   **分布式路由**：Web 路由（Axum）分散在各个模块中定义，启动时自动收集聚合，无需在 `main` 中手动挂载。
*   **声明式定时任务**：通过 `#[cron_job]` 宏直接在函数上定义定时任务。
*   **分层配置系统**：支持 `application.toml` 及多环境配置（Profile）自动合并。

---

## 🚀 快速开始

下面是一个包含 **依赖注入**、**Web 接口**、**定时任务** 和 **配置读取** 的完整示例。

### 1. 配置文件 (`resources/application.toml`)

```toml
[app]
name = "MyAwesomeApp"

[logger]
level = "DEBUG"
with_thread_id = true
with_thread_name = true
enable_console = true

[web]
port = 8080
binding = "0.0.0.0"

# 自定义数据库配置
[database]
url = "postgres://user:pass@localhost:5432/mydb"
```

### 2. 代码实现 (`main.rs`)

```rust
use serde::{Deserialize, Serialize};
use simple_starter_core::tracing::info;
use simple_starter_core::{AppCoreUtil, Application, anyhow::Result};
use simple_starter_macro::{component, cron_job, get, json_response, provider};
use simple_starter_web::{JsonResponse, WebPlugin, axum, json_response_wrap};
use std::sync::Arc;

// --- 1. 定义配置结构 ---
#[derive(Deserialize)]
struct DbConfig {
    url: String,
}

// --- 2. 定义业务组件 (Struct形式) ---
// destroy_method 指定销毁时调用的方法
#[component(name = "DatabaseCom", destroy_method = "disconnect")]
#[derive(Debug)]
struct Database {
    url: String,
}

impl Database {
    async fn disconnect(self) {
        info!("🔌 断开数据库连接: {}", self.url);
    }
}

// --- 3. 工厂模式注册组件 (Provider形式) ---
// 适用于第三方库对象或需要复杂初始化的对象
// 参数自动注入上下文或其他组件
#[provider] // 自动推断返回类型 Database 为组件类型
async fn db_factory() -> Database {
    // 从全局配置读取
    let cfg: DbConfig =
        AppCoreUtil::get_config_to_struct("database").expect("Failed to load database config");
    info!("🔗 连接数据库: {}", cfg.url);
    Database { url: cfg.url }
}

// --- 4. 依赖注入示例 ---
#[component(init_method = "init")]
struct UserService {
    // 按类型自动注入 Database 组件 (必须包裹在 Arc 中)
    #[inject]
    db: Arc<Database>,
}

impl UserService {
    async fn init(&self) -> Result<()> {
        info!("✅ UserService 初始化完成，依赖的 DB URL: {}", self.db.url);
        Ok(())
    }

    fn get_user_name(&self, id: u32) -> String {
        format!("User-{}", id)
    }
}

// --- 5. Web 路由与控制器 ---
#[derive(Serialize, Debug)]
struct UserDto {
    id: u32,
    name: String,
}

// 自动注册 GET 路由，支持 axum 的 extractors
#[get("/api/users/{id}")]
#[json_response] // 自动将返回值包装为 Json
async fn get_user(
    axum::extract::Path(id): axum::extract::Path<u32>,
    // 注意：Web Handler 中暂不支持直接属性注入，需手动获取
) -> JsonResponse {
    // 获取组件单例
    let user_service = AppCoreUtil::get_component::<UserService>().unwrap();

    // 使用宏统一处理响应格式 (code, msg, data)
    json_response_wrap!(function_name = "get_user", {
        if id == 0 {
            return Err(SimpleAppWebError::new(400, "Invalid User ID"));
        }
        Ok(UserDto {
            id,
            name: user_service.get_user_name(id),
        })
    })
}

// --- 6. 定时任务 ---
#[cron_job("*/5 * * * * *")] // 每5秒执行
async fn heartbeat_task() {
    info!("💓 系统心跳检查...");
}

// --- 7. 主程序入口 ---
fn main() {
    Application::new()
        // 注册 Web 插件 (加载路由、启动 HTTP 服务)
        .register_plugin(WebPlugin::new())
        .add_startup_hook(async {
            // 手动添加组件
            let user = UserDto {
                id: 2,
                name: "Bob".into(),
            };
            AppCoreUtil::register_component(
                user,
                Some(move |user: UserDto| async move {
                    info!("User destroyed: {:?}", user);
                }),
            )
                .unwrap();
        })
        // 添加关闭钩子
        .add_shutdown_hook(async {
            info!("🚀 系统关闭钩子执行！！！");
        })
        // 运行应用 (阻塞主线程)
        .run();
}
```

---

## 🔄 启动流程图

`simple-starter` 严格遵循以下生命周期顺序，确保依赖就绪后再执行业务逻辑。

```mermaid
graph TD
    Start[Application::run] --> LoadConfig[1. 加载配置 application.toml]
    LoadConfig --> InitLog[2. 初始化 Tracing 日志系统]
    InitLog --> InitRuntime[3. 初始化 Tokio Runtime]

    subgraph ComponentLifecycle [组件生命周期]
        Scan[4. Inventory 扫描元数据]
        TopoSort[5. 计算依赖拓扑排序]
        Create["6. 执行组件 create()"]
        Init["7. 执行组件 init()"]
    end

    InitRuntime --> ComponentLifecycle

    subgraph Plugins [插件系统]
        PluginSort[8. 插件依赖排序]
        PluginInit["9. 执行 Plugin::init()"]
        WebStart[WebPlugin: 启动 HTTP Server]
    end

    ComponentLifecycle --> Plugins
    Plugins --> Hooks[10. 执行 Startup Hooks]
    Hooks --> Scheduler[11. 启动 Cron 调度器]

    Scheduler --> Running(("运行中 / 等待信号"))

    Running --> Signal[收到退出信号 Ctrl+C]

    subgraph Shutdown [优雅停机]
        StopTasks[12. 停止后台任务]
        ShutdownHooks[13. 执行 Shutdown Hooks]
        PluginStop[14. 逆序关闭插件]
        DestroyCom["15. 逆序执行组件 destroy()"]
    end

    Signal --> Shutdown
    Shutdown --> End[退出]
```

---

## 🛠 关键实现原理

### 1. 组件收集与注册 (Inventory Pattern)
Rust 在编译期无法反射获取所有类型。本库利用 `inventory` crate 和 `ctor` 机制。
*   **原理**：`#[component]` 宏会为每个结构体生成一个静态的 `ComponentProcessorFactory` 实例，并标记为 `inventory::submit!`。
*   **运行时**：应用启动时，遍历所有提交的 Factory，收集组件的构造函数、类型ID和依赖关系。

### 2. 依赖注入与拓扑排序 (DI & Topological Sort)
组件之间存在依赖关系（如 Service 依赖 Database）。
*   **依赖声明**：宏解析 struct 字段上的 `#[inject]` 属性，记录依赖的类型名称。
*   **排序算法**：系统构建一个依赖有向图，使用 **Kahn 算法** 进行拓扑排序。
*   **实例化**：按照排序后的顺序依次调用 `create`（实例化）和 `init`（业务初始化）。`init` 阶段可以通过 `Arc` 安全地获取已创建的依赖组件。

### 3. 分布式路由 (Distributed Routing)
传统的 Web 框架通常需要在 `main` 函数中集中 `app.route(...)`。
*   **解耦**：`#[get/post]` 宏将处理函数包装为 `RouteFactory` 并提交到 inventory。
*   **聚合**：`WebPlugin` 在初始化时，自动收集所有分散的 `RouteFactory`，构建出一个完整的 `Axum Router`，实现 Controller 定义与注册的彻底解耦。

### 4. 统一响应封装 (Response Wrapping)
*   **宏实现**：`json_response_wrap!` 宏利用 Rust 的模式匹配，执行用户的 `Result` 代码块。
*   **自动转换**：
    *   `Ok(data)` -> `{ code: 200, message: "...", data: ... }`
    *   `Err(e)` -> 自动捕获错误堆栈日志，并返回 `{ code: 500, message: "服务器错误" }` (可自定义错误码)。

---

## 📂 核心宏说明

| 宏 | 作用 | 示例 |
| :--- | :--- | :--- |
| `#[component]` | 标记结构体为组件，纳入生命周期管理。 | `#[component(name="auth", init_method="start")]` |
| `#[provider]` | 将函数标记为组件工厂，用于创建复杂对象。 | `#[provider] async fn create_db() -> Database` |
| `#[inject]` | 标记字段需要注入依赖。 | `#[inject] db: Arc<Database>` |
| `#[cron_job]` | 注册定时任务。 | `#[cron_job("0 * * * * *")]` |
| `#[get/post...]` | 注册 HTTP 路由。 | `#[get("/user")]` |
| `#[json_response]` | 转换返回值为 Json 格式。 | `async fn handler() -> JsonResponse` |