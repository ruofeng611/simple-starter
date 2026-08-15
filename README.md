# simple-starter

**simple-starter** 是一个模块化的 Rust 应用程序启动框架。它提供一套基于**宏（Macros）**与**依赖注入（DI）**的机制，自动管理组件生命周期、依赖关系、配置加载、Web 路由注册与安全校验，让你专注于业务逻辑的实现。

## 主要特性

- **自动依赖注入**：`#[component]`、`#[provider]`、`#[configuration]` 三种组件注册方式，支持按类型、按名称注入
- **Trait Object 注入**：`#[injectable]` + `#[inject]` 支持 `Arc<dyn Trait>`（唯一实现）、按名称指定实现、`Vec<Arc<dyn Trait>>`（收集全部实现）
- **智能生命周期管理**：自动计算组件依赖拓扑，按序 create → init，退出时逆序 destroy
- **条件注册**：`condition = ComponentCondition::on_missing_trait::<dyn T>()` 实现"默认实现 + 用户覆盖"
- **分布式路由**：Web 路由分散定义，启动时自动收集聚合，无需集中挂载
- **声明式定时任务**：`#[cron_job]` 直接在函数上定义定时任务
- **分层配置系统**：`application.toml` 与多环境 Profile 自动合并
- **插件系统**：`assemble` / `components_ready` / `finalize` 三阶段生命周期，插件间通过扩展上下文解耦协作
- **事件系统**：Spring 风格事件发布/监听，监听器自动收集、按事件类型分桶分派
- **安全能力**：编译期资源收集、运行时白名单、用户认证与权限校验

## 模块导航

| 模块 | 说明 | 文档 |
|---|---|---|
| **simple-starter-core** | 运行时核心：组件模型、依赖注入、插件系统、配置管理、事件系统、任务调度 | [README](./simple-starter-core/README.md) |
| **simple-starter-macro** | 过程宏集合：组件注册、依赖注入、路由挂载等声明式注解的代码展开 | [README](./simple-starter-macro/README.md) |
| **simple-starter-web** | Web 插件：Axum 集成、路由自动收集、统一 JSON 响应、监听器扩展 | [README](./simple-starter-web/README.md) |
| **simple-starter-security** | 安全插件：编译期资源收集、白名单放行、用户认证与权限校验 | [README](./simple-starter-security/README.md) |

## 模块职责与依赖关系

```
simple-starter-macro（纯过程宏，不依赖任何模块）
        ↑
simple-starter-core（运行时引擎，依赖并重导出核心宏）
        ↑
simple-starter-web（Axum Web 集成，依赖 core + macro）
        ↑
simple-starter-security（安全中间件，依赖 core + web）
```

- **simple-starter-core**：框架的核心引擎，不依赖任何业务模块。采用 serde 模式依赖并重导出核心宏，用户只需依赖 core 即可使用 `#[component]` 等宏。
- **simple-starter-macro**：纯过程宏 crate，展开代码通过绝对路径（`::simple_starter_core::...`、`::simple_starter_web::...`、`::simple_starter_security::...`）引用运行时，自身零依赖。注意：过程宏 crate 只能导出宏，普通类型由各自模块导出。
- **simple-starter-web**：依赖 core，重导出 Web 相关宏。
- **simple-starter-security**：依赖 core + web，重导出安全相关宏。

用户侧依赖规则：使用核心能力（组件、DI、事件、定时任务）只依赖 `simple-starter-core`；使用 Web 路由宏依赖 `simple-starter-web`；使用安全宏依赖 `simple-starter-security`；均无需直接依赖 `simple-starter-macro`。

## 快速开始

一个最小 Web 应用：

```rust
use simple_starter_core::{component, inject};
use simple_starter_core::Application;
use simple_starter_web::{get_mapping, json_response_wrap, rest_controller, JsonResponse, WebPlugin};
use std::sync::Arc;

// 业务组件：自动注册、生命周期托管
#[component]
struct GreetingService;

// Controller 组件：通过 #[inject] 自动装配依赖
#[component]
struct HelloController {
    #[inject]
    greeting: Arc<GreetingService>,
}

// REST 控制器：impl 块级路由注册
#[rest_controller("/api")]
impl HelloController {
    #[get_mapping("/hello")]
    async fn hello(&self) -> JsonResponse {
        json_response_wrap!(message = "success", { Ok("hello world".to_string()) })
    }
}

fn main() {
    Application::new()
        .register_plugin(WebPlugin::new())
        .run();
}
```

> 各模块 README 均内置完整示例（组件注入、trait 对象注入、插件协作、Web 路由、安全校验、事件系统），可参照实现自己的应用。

## 目录结构

```
simple-starter/
├── simple-starter-core/      # 运行时核心（重点模块，见其 README）
├── simple-starter-macro/     # 过程宏集合
├── simple-starter-web/       # Web 插件
├── simple-starter-security/  # 安全插件
└── README.md                 # 本文档（模块导航）
```
