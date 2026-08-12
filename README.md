这是一个名为 **simple-starter** 的 Rust 应用程序启动库说明文档。它旨在通过宏和依赖注入机制，简化复杂 Rust 应用的构建、生命周期管理和模块解耦。

---

# simple-starter

**simple-starter** 是一个模块化的 Rust 应用程序启动库。它提供了一套基于 **宏（Macros）** 和 **依赖注入（DI）** 的机制，用于自动管理组件生命周期、依赖关系、配置加载以及 Web 路由注册，让你专注于业务逻辑的实现。

## ✨ 主要特性

*   **自动依赖注入**：通过 `#[component]` 和 `#[inject]` 宏实现组件的自动注册与装配，支持按类型或名称注入。
*   **Trait Object 注入**：通过 `#[injectable]` 注册 trait 实现，支持 `Arc<dyn Trait>` 按 trait、按名称注入及 `Vec<Arc<dyn Trait>>` 收集全部实现，TypeId 直接匹配。
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
name = "Application"

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
url = "postgres://postgres:postgres@localhost:5432/practice"
```

### 2. 依赖项
```toml
[package]
name = "temp"
version = "0.1.0"
edition = "2024"

[dependencies]
simple-starter-core = { path = "../simple-starter/simple-starter-core" }
simple-starter-macro = { path = "../simple-starter/simple-starter-macro" }
simple-starter-web = { path = "../simple-starter/simple-starter-web" }
serde = { version = "1.0.228", features = ["derive"] }
toml = "0.9.8"
sea-orm = { version = "1.1.19", features = ["sqlx-postgres", "runtime-tokio-rustls", "macros"] }
```

### 3. 代码实现 
(`main.rs`)
```rust
mod entity;

use serde::{Deserialize, Serialize};
use simple_starter_core::tracing::info;
use simple_starter_core::{anyhow, AppCoreUtil, Application};
use simple_starter_macro::{component, configuration, cron_job, get, json_response, provider};
use simple_starter_web::axum::extract::State;
use simple_starter_web::{JsonResponse, WebPlugin, axum, json_response_wrap};
use std::sync::Arc;
use std::time::Duration;
use sea_orm::{ActiveModelTrait, ConnectOptions, Database, DatabaseConnection, DbErr, EntityTrait, Set};
use toml::{Value, toml};
use crate::entity::prelude::Student;
use crate::entity::student;

// --- 1. 定义配置组件 ---
#[derive(Deserialize, Debug)]
#[configuration("constant")]
pub struct ConstantComponent {
    pub test_name: String,
    pub any_str: String,
}

#[derive(Deserialize)]
#[configuration("database")]
struct DbConfig {
    url: String,
}

// --- 2. 定义业务组件 (Struct形式) ---
// destroy_method 指定销毁时调用的方法
#[component(name = "DatabaseComponent", destroy_method = "disconnect")]
#[derive(Debug)]
struct DatabaseComponent {
    url: String,  // 对于组件中没有自动注入的字段，使用 Default trait 提供默认值
}

impl DatabaseComponent {
    async fn disconnect(self) -> anyhow::Result<()> {
        info!("🔌 断开数据库连接: {}", self.url); // 这里输出为字符串类型的默认值（空串）
        Ok(())
    }
}

async fn db_destroy(db: DatabaseConnection) -> anyhow::Result<()> {
    // 显示drop，不过实际上一般不需要
    std::mem::drop(db);
    Ok(())
}

// --- 3. 工厂模式注册组件 (Provider形式) ---
// 适用于第三方库对象或需要复杂初始化的对象
// 参数自动注入上下文或其他组件

//或者 #[provider(destroy_method = async |db| -> anyhow::Result<()> {
//         std::mem::drop(db);
//         Ok(())
//     }
// )]
#[provider(destroy_method = db_destroy)] // 自动推断返回类型 DatabaseConnection 为组件类型
async fn db_factory(cfg: Arc<DbConfig>) -> Result<DatabaseConnection, DbErr> {
    let mut opt = ConnectOptions::new(cfg.url.clone(), );
    opt.max_connections(10)
        .min_connections(5)
        .connect_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(60));
    let db: DatabaseConnection = Database::connect(opt).await?;
    Ok(db)
}

// --- 4. 依赖注入示例 ---
#[component(init_method = "init")]
struct StudentService {
    // 按类型自动注入DatabaseConnection 组件 (必须包裹在 Arc 中)
    #[inject]
    db: Arc<DatabaseConnection>,
}

impl StudentService {
    async fn init(&self) -> anyhow::Result<()> {
        // 这里插入一条数据
        let student = student::ActiveModel {
            id: Set(12345),
            student_id: Set("S2025001".into()),
            name: Set("张三".into()),
            ..Default::default() // 其他字段用 ActiveModel 的 Default（即 NotSet）
        };
        student.insert(self.db.as_ref()).await?;
        Ok(())
    }

    async fn get_student_name(&self, id: i64) -> Option<String> {
        let res = Student::find_by_id(id).one(self.db.as_ref()).await;
        match res {
            Ok(Some(model)) => Some(model.name),
            _ => None, // 包括 NotFound 和 DB error
        }
    }
}

// --- 5. Web 路由与控制器 ---
#[derive(Serialize, Debug)]
struct StudentVO {
    id: i64,
    name: String,
}

// 自动注册 GET 路由，支持 axum 的 extractors
#[get(path = "/student/{id}", state = AppCoreUtil::get_component::<StudentService>().unwrap())]
#[json_response] // 自动将返回值包装为 Json
async fn get_student_name(
    axum::extract::Path(id): axum::extract::Path<i64>,
    State(student_service): State<Arc<StudentService>>,
    // 注意：Web Handler 中暂不支持直接属性注入，需手动获取
) -> JsonResponse {
    // 使用宏统一处理响应格式 (code, msg, data)
    json_response_wrap!(function_name = "根据学生id获取学生姓名", {
        if id == 0 {
            return Err(SimpleAppWebError::new(400, "无效的学生id"));
        }
        Ok(StudentVO {
            id,
            name: student_service.get_student_name(id).await.ok_or_else(|| SimpleAppWebError::new(404, "未找到该id相关的学生姓名"))?,
        })
    })
}

// 手动添加的路由
#[json_response]
async fn manual_router() -> () {
    // 输出配置组件信息
    info!("{:?}", AppCoreUtil::get_component::<ConstantComponent>().unwrap());
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
        .register_plugin(
            WebPlugin::new()
                // 手动添加路由
                .add_manual_router_factory(|| {
                    axum::Router::new().route("/manual", axum::routing::get(manual_router))
                }),
        )
        // 添加默认配置
        .add_default_config(Value::Table(toml! {
            [constant]
            test_name = "simple-starter"
            any_str = "This is a test string"

            [web]
            base_path = "/api"
        }))
        .add_startup_hook(async {
            // 手动添加组件
            let database_component = DatabaseComponent {
                url: "手动添加的组件url".to_string(),
            };
            AppCoreUtil::register_component_with_name(
                database_component,
                "DatabaseComponentManual",
                Some(move |db: DatabaseComponent| async move {
                    db.disconnect().await
                }),
            )?;
            Ok(())
        })
        // 添加关闭钩子
        .add_shutdown_hook(async {
            info!("🚀 系统关闭钩子执行！！！");
            Ok(())
        })
        // 运行应用 (阻塞主线程)
        .run();
}
```

(`src/entity/student.rs`) (sea-orm生成)
```rust
//! `SeaORM` Entity, @generated by sea-orm-codegen 1.1.19

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize, Default)]
#[sea_orm(table_name = "student")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    #[sea_orm(unique)]
    pub student_id: String,
    pub name: String,
    pub gender: Option<String>,
    pub birth_date: Option<Date>,
    #[sea_orm(unique)]
    pub email: Option<String>,
    pub phone: Option<String>,
    pub major: Option<String>,
    pub grade: Option<String>,
    pub class_name: Option<String>,
    pub enrollment_date: Option<Date>,
    pub status: Option<String>,
    pub created_at: Option<DateTime>,
    pub updated_at: Option<DateTime>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
```
---

## 🔄 启动流程图

`simple-starter` 严格遵循以下生命周期顺序，确保依赖就绪后再执行业务逻辑。

```mermaid
graph TD
%% Nodes
    Start([Application::run]) --> ConfigLoad

    subgraph S_Config [1. 配置与日志加载]
        ConfigLoad[加载全局配置] --> LoadBase["加载 application.toml"]
        LoadBase --> CheckProfile{是否存在 Profile?}
        CheckProfile -- 是 --> LoadProfile["加载 application-{profile}.toml"]
        LoadProfile --> MergeConfig[合并配置: 默认 + 基础 + Profile]
        CheckProfile -- 否 --> MergeConfig
        MergeConfig --> InitTracing[初始化 Tracing 日志系统]
        InitTracing --> SetupLayers[设置日志层 & 文件守卫]
    end

    subgraph S_Runtime [2. 运行时初始化]
        SetupLayers --> InitRuntime[初始化 Tokio 运行时]
        InitRuntime --> CheckFactory{是否有自定义工厂?}
        CheckFactory -- 是 --> UseFactory[使用自定义运行时工厂]
        CheckFactory -- 否 --> BuildRuntime[构建 多线程/单线程 运行时]
    end

subgraph S_Start [3. 启动阶段]
UseFactory --> CallStart["调用 self.start()"]
BuildRuntime --> CallStart

CallStart --> CheckComps{是否存在组件?}

%% Component Loading Logic from component_loader.rs
subgraph S_Components [组件加载流程]
CheckComps -- 是 --> CompLoad[加载组件仓库]
CompLoad --> CompReg[注册并检查名称唯一性]
CompReg --> CompTopo[计算依赖拓扑顺序]
CompTopo --> CompCycle{检测到循环依赖?}
CompCycle -- 是 --> Error[返回错误]
CompCycle -- 否 --> CompCreate["循环: processor.create()"]
CompCreate --> CompInit["循环: processor.init()"]
end

CheckComps -- 否 --> CheckPlugins
CompInit --> CheckPlugins{是否存在插件?}

%% Plugin Logic
subgraph S_Plugins [插件加载流程]
CheckPlugins -- 是 --> PluginSort[按依赖排序插件]
PluginSort --> PluginInit["循环: plugin.init()"]
end

CheckPlugins -- 否 --> StartHooks
PluginInit --> StartHooks[执行启动钩子 Startup Hooks]
end

subgraph S_Execution [4. 主运行循环]
StartHooks --> CheckMainLoop{是否有自定义主循环?}

%% Custom Main Loop Path
CheckMainLoop -- 是 (如 GUI) --> SpawnCore[后台派发 App 核心管理任务]
SpawnCore --> UserLoop[执行用户自定义主循环钩子]
UserLoop --> UserShutdown["用户手动调用 shutdown()"]

%% Default Main Loop Path
CheckMainLoop -- 否 (默认) --> BlockCore[阻塞等待 App 核心任务]

subgraph S_CoreTask [核心任务逻辑]
BlockCore --> SchedCreate[创建并启动 Cron 调度器]
SchedCreate --> TaskSpawn[派发注册的异步任务]
TaskSpawn --> WaitSignal[等待退出信号 Ctrl+C / SIGTERM]
end

WaitSignal --> AutoShutdown[触发自动关闭流程]
end

subgraph S_Shutdown [5. 关闭流程]
UserShutdown --> ShutdownStart["执行 shutdown()"]
AutoShutdown --> ShutdownStart

ShutdownStart --> CancelToken[取消异步任务 Token]
CancelToken --> WaitCore[等待核心任务结束]
WaitCore --> DownHooks[执行关闭钩子 Shutdown Hooks]
DownHooks --> PluginDown["插件关闭 (逆序)"]
PluginDown --> CompDown["组件销毁 (逆序)"]
CompDown --> End([程序退出])
end

%% Styles
style Start fill:#f9f,stroke:#333,stroke-width:2px
style End fill:#f9f,stroke:#333,stroke-width:2px
style Error fill:#f00,stroke:#333,color:#fff
style S_Config fill:#e1f5fe,stroke:#01579b
style S_Components fill:#fff3e0,stroke:#e65100
style S_Shutdown fill:#ffebee,stroke:#b71c1c
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

### 5. Trait Object 注入 (Trait Object DI)
*   **实现注册**：`#[injectable]` 作用于 `impl Trait for Type`，生成 `TraitImplRegistration`（trait TypeId + 实现 TypeId + 类型擦除 accessor）提交到 inventory。
*   **依赖声明**：`#[inject]` 支持三种形式 —— `Arc<dyn Trait>`（按 trait 获取唯一实现）、`#[inject(name = "X")] Arc<dyn Trait>`（按名称获取）、`Vec<Arc<dyn Trait>>`（收集全部实现）。
*   **排序解析**：trait 依赖以 `fn() -> TypeId` 函数指针声明，拓扑排序时通过 TypeId 直接展开为所有实现组件，无需字符串桥接（跨 crate 同名 trait 不冲突）。
*   **运行时获取**：`AppCoreUtil::get_component_by_trait` / `get_component_by_trait_and_name` / `get_components_by_trait`，从 trait 对象缓存读取，不扫描组件仓库（避免 create 阶段死锁）。

---

## 📂 核心宏说明

| 宏                  | 作用                                  | 示例                                               |
|:-------------------|:------------------------------------|:-------------------------------------------------|
| `#[component]`     | 标记结构体为组件，纳入生命周期管理。                  | `#[component(name="auth", init_method="start")]` |
| `#[provider]`      | 将函数标记为组件工厂，用于创建复杂对象。                | `#[provider] async fn create_db() -> Database`   |
| `#[configuration]` | 将结构体标记为配置组件，从全局配置文件中读取对应路径的数据进行反序列化 | `#[configuration("constant")]`                   |
| `#[inject]`        | 标记字段需要注入依赖。                         | `#[inject] db: Arc<Database>`                    |
| `#[injectable]`    | 注册 trait 与实现类的映射，支持 trait object 注入。   | `#[injectable] impl FileParser for JsonParser`   |
| `#[cron_job]`      | 注册定时任务。                             | `#[cron_job("0 * * * * *")]`                     |
| `#[get/post...]`   | 注册 HTTP 路由。                         | `#[get("/user")]`                                |
| `#[json_response]` | 转换返回值为 Json 格式。                     | `async fn handler() -> JsonResponse`             |