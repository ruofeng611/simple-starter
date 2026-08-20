# simple-starter-core

`simple-starter-core` 是 simple-starter 框架的**运行时核心**：提供配置管理、组件模型、依赖注入、插件系统、事件系统、任务调度与生命周期管理。web / security 等插件与用户应用都构建在它之上。

## 一、基本原理

### 1. 组件模型

组件是框架管理的最小单元。每个组件经历完整的生命周期：

1. **注册**：`#[component]` / `#[provider]` / `#[configuration]` 宏在编译期生成静态注册元数据（`ComponentProcessorFactory`），通过 `inventory` 自动收集（Rust 无反射，编译期静态收集替代运行时扫描）。
2. **条件过滤**：注册期统一评估 `condition` 表达式，不满足者不参与装配。
3. **依赖排序**：基于组件声明的四类依赖（名称 / trait / 类型 / primary）构建有向图，Kahn 拓扑排序得到创建顺序；存在依赖环时排序结果数小于组件总数，报错并给出环路径。
4. **创建**：按依赖序调用构造函数，非注入字段使用 `Default::default()` 填充。
5. **初始化**：全部组件创建完成后，按创建序执行 `init_method`（init 阶段可安全获取依赖组件）。方法签名 `async fn init(&self)`：以 `Arc<T>` 共享引用调用，实例所有权仍在仓库，仅能读取/借用自身。
6. **销毁**：退出时按创建序的逆序执行 `destroy_method`。方法签名 `async fn destroy(self)`：与 init 不同，以「有所有权」的实例 `T` 调用，所有权已从仓库移出，可消费字段、取出内部资源（要求此时无其他地方持有该实例，即 `Arc` 计数为 1）。

### 2. 依赖注入

注入目标支持三种形态，依赖声明方式决定注入策略：

| 字段/参数形态 | 注入语义 | 依赖声明 |
|---|---|---|
| `Arc<T>`（无名称） | 按具体类型注入（短名快速路径 + 按类型唯一实例兜底，自定义名也能命中） | 类型 TypeId |
| `Arc<T>`（有名称） | 按组件名精确注入 | 组件名 |
| `Arc<dyn Trait>`（无名称） | 按 trait 注入唯一实现（多个实现报错） | trait TypeId |
| `Arc<dyn Trait>`（有名称） | 按 trait + 名称注入指定实现 | 组件名 |
| `Vec<Arc<dyn Trait>>` | 收集该 trait 的全部实现 | trait TypeId |
| `Arc<T>` + `#[inject_primary]` | 注入该类型的 primary（首要）实例 | 类型 TypeId |

### 3. trait 对象注入与"trait 还原"原理

`Arc<dyn Trait>` 注入的关键难点是：trait 对象是**胖指针**（数据指针 + vtable 指针），而 Rust 的 vtable 布局未规范化，无法仅凭类型信息从零构造。

框架的解法是"**记录 + 还原**"两步：

1. **写入侧（coercion 瞬间记录）**：`#[injectable]` 宏生成 accessor，在 `Arc<ConcreteType>` 被向上转型（upcasting coercion）为 `Arc<dyn Injectable>` 的那一刻，编译器算出真实的 vtable——accessor 顺势把这个 vtable 与数据指针一起写入缓存条目 `TraitObjectEntry`（vtable 是 `'static` 只读数据，可安全长期保存）。
2. **读取侧（拼回胖指针）**：注入时取出缓存的"数据指针 + 记录 vtable"，拼回 `Arc<dyn Trait>` 胖指针，完成 trait 还原。

配合 `TypeId` 直接匹配（trait 依赖在宏展开期生成 `TypeId::of::<dyn Trait>()`，const fn 编译期求值），全程无字符串桥接，跨 crate 同名 trait 不冲突。

### 4. 插件系统

插件是框架级的扩展单元，按拓扑顺序（`dependencies()` 声明依赖）执行三阶段生命周期，与组件加载严格编排：

```
插件排序 → assemble（装配期）→ 组件加载 → components_ready（组件就绪期）→ finalize（收尾期）→ 启动钩子
```

| 周期 | 时机 | 职责 |
|---|---|---|
| `assemble` | 组件加载前 | 将扩展注册表放入 `Application` 上下文，供其他插件注册扩展 |
| `components_ready` | 组件全部 create + init 后 | 从组件仓库获取协作接口实例，构建插件协作结构（如中间件状态） |
| `finalize` | 所有插件就绪后 | 消费扩展注册表、构建并启动服务 |

### 5. 配置分层

配置按优先级从低到高合并：**硬编码默认 → 插件默认配置 → 用户代码默认配置 → application.toml → application-{profile}.toml**。通过 `AppCoreUtil::get_config_value_by_path("a.b.c")` 按点分路径读取，或 `get_config_to_struct::<T>("prefix")` 反序列化为结构体。

### 6. 条件注册

`#[component(condition = ...)]` / `#[provider(condition = ...)]` 支持注册期条件评估：条件不满足的组件自动退位。这是"插件提供默认实现、用户覆盖"的核心机制，详见下文 `ComponentCondition`。

## 二、导出的用户可用组件与 API

### 1. Application（应用构建器）

`Application` 是应用入口，采用链式构建 + `run()` 启动：

```rust
fn main() {
    Application::new()
        .register_plugin(MyPlugin::new())        // 注册插件（可多次）
        .add_default_config(toml! { /* ... */ }) // 代码内默认配置
        .add_startup_hook(async { /* ... */ Ok(()) })  // 启动钩子
        .add_shutdown_hook(async { /* ... */ Ok(()) }) // 关闭钩子
        .run();
}
```

| 方法 | 说明 |
|---|---|
| `new()` | 创建实例 |
| `register_plugin(p)` | 注册插件（依赖关系自动拓扑排序） |
| `add_default_config(v)` | 添加代码内默认配置（优先级高于插件默认、低于配置文件） |
| `add_startup_hook(f)` / `add_shutdown_hook(f)` | 启动/关闭钩子（async 闭包） |
| `set_tokio_runtime_factory(f)` | 自定义 Tokio 运行时工厂 |
| `add_log_layer_factory(l)` | 追加自定义 tracing layer |
| `register_task_spawn_factory(f)` | 注册异步后台任务（接收 CancellationToken，优雅退出） |
| `set_main_loop_hook(f)` | 自定义主循环（GUI 框架接管主线程，随后手动调 `shutdown()`） |
| `insert_extension(v)` / `get_extension::<T>()` / `get_extension_mut::<T>()` / `remove_extension::<T>()` | 扩展上下文（AnyMap），插件间传递协作数据 |
| `run()` | 启动应用（阻塞） |
| `shutdown()` | 手动触发关闭（取消任务 → 关闭钩子 → 插件逆序关闭 → 组件逆序销毁） |

### 2. AppCoreUtil（运行时获取工具）

| API | 说明 |
|---|---|
| `get_config_value_by_path("a.b")` | 按点分路径读取配置值 |
| `get_config_to_struct::<T>("prefix")` | 按前缀反序列化配置为结构体 |
| `get_component::<T>()` | 按类型获取组件（短名快速路径，自定义名时按唯一实例兜底） |
| `get_component_by_name::<T, _>("name")` | 按名称获取组件 |
| `get_primary_component::<T>()` | 按类型获取 primary 实例（未声明 primary 时回退为唯一实例） |
| `get_component_by_trait::<dyn Trait>()` | 按 trait 获取唯一实现 |
| `get_component_by_trait_and_name::<dyn Trait>("name")` | 按 trait + 名称获取指定实现 |
| `get_components_by_trait::<dyn Trait>()` | 收集 trait 全部实现 `Vec<Arc<dyn Trait>>` |

### 3. 核心宏（本模块重导出，无需依赖 macro）

| 宏 | 作用 |
|---|---|
| `#[component]` | 标记结构体为组件：`name` / `init_method` / `destroy_method` / `condition` |
| `#[provider]` | 标记函数为组件工厂（适用于第三方类型或复杂初始化） |
| `#[primary]` | 配合 `#[provider]` 声明返回类型的首要实例 |
| `#[configuration]` | 标记结构体为配置组件，从全局配置反序列化 |
| `#[inject]` | 标记字段/参数注入依赖 |
| `#[inject_primary]` | 标记字段/参数按 primary 实例注入 |
| `#[injectable]` | 标记 trait 实现，注册 trait → 实现映射 |
| `#[cron_job]` | 声明式定时任务 |
| `#[event_listener]` | 声明式事件监听器 |

```rust
use simple_starter_core::{component, configuration, cron_job, inject, injectable, provider, primary};

// 配置组件：从 TOML 的 [database] 段反序列化
#[derive(serde::Deserialize)]
#[configuration("database")]
struct DbConfig { url: String }

// 函数工厂：适用于第三方类型 DatabaseConnection；参数自动按类型注入
#[provider]
async fn db_factory(cfg: std::sync::Arc<DbConfig>) -> anyhow::Result<DatabaseConnection> {
    Database::connect(&cfg.url).await
}

// 多实例 + primary：mainDb 是按类型获取时的首要实例
#[provider(name = "mainDb")]
#[primary(name = "mainDb")]
async fn main_db() -> anyhow::Result<DatabaseConnection> { /* ... */ }

// 结构体组件：字段注入依赖，init_method 在依赖就绪后执行
#[component(init_method = "init")]
struct UserService {
    #[inject]
    db: std::sync::Arc<DatabaseConnection>,
}
impl UserService {
    async fn init(&self) -> anyhow::Result<()> { Ok(()) }
}

// 定时任务
#[cron_job("*/5 * * * * *")]
async fn heartbeat_task() { /* 每 5 秒执行 */ }
```

### 4. Plugin trait（自定义插件）

```rust
#[async_trait::async_trait]
impl Plugin for MyPlugin {
    fn name(&self) -> &'static str { "MyPlugin" }
    fn dependencies(&self) -> &[&'static str] { &["WebPlugin"] }  // 可选：声明依赖
    fn default_config(&self) -> toml::Value { /* 可选：插件默认配置 */ }

    async fn assemble(&mut self, ctx: &mut Application) -> anyhow::Result<()> { Ok(()) }
    async fn components_ready(&mut self, ctx: &mut Application) -> anyhow::Result<()> { Ok(()) }
    async fn finalize(&mut self, ctx: &mut Application) -> anyhow::Result<()> { Ok(()) }
}
```

### 5. Injectable trait（可注入 trait 的 supertrait）

所有可注入 trait 必须继承 `Injectable`（`Any + Send + Sync` 的 blanket impl），使所有 trait 对象可统一类型擦除：

```rust
use simple_starter_core::Injectable;

pub trait FileParser: Injectable {
    fn parse(&self, content: &str) -> anyhow::Result<Vec<String>>;
}
```

### 6. ComponentCondition（条件注册）

| 条件 | 语义 |
|---|---|
| `on_missing_type::<T>()` | 无其他已注册组件是具体类型 `T`（默认实现 + 用户覆盖） |
| `on_missing_trait::<dyn Trait>()` | 无其他已注册组件实现该 trait（trait 替换默认实现） |
| `on_property("a.b")` | 全局配置存在该点分路径键 |
| `on_property_eq("a.b", "v")` | 全局配置该键的字符串值等于 `v` |
| `Custom(fn(&ConditionContext) -> bool)` | 用户自定义条件 |

```rust
// 默认实现：仅当用户未提供 CacheService 实现时才注册
#[component(condition = simple_starter_core::ComponentCondition::on_missing_trait::<dyn CacheService>())]
pub struct DefaultCacheService;
```

### 7. 事件系统（Spring 风格发布/监听）

```rust
use simple_starter_core::{event_listener, inject, AppEvent, EventPublisherExt, component};

// 1. 定义事件：实现 AppEvent 标记 trait
#[derive(Debug)]
struct UserLoginEvent { user_id: String }

// 2. 定义监听器：impl 块 + #[event_listener]，随组件注册自动收集
#[component]
struct LoginListener;
#[event_listener]
#[async_trait::async_trait]
impl EventListener<UserLoginEvent> for LoginListener {
    async fn on_event(&self, event: &UserLoginEvent) -> anyhow::Result<()> { Ok(()) }
}

// 3. 发布：注入 EventPublisher，调用 publish_event
#[component]
struct LoginService {
    #[inject]
    publisher: std::sync::Arc<dyn EventPublisher>,
}
impl LoginService {
    async fn login(&self) -> anyhow::Result<()> {
        self.publisher.publish_event(UserLoginEvent { user_id: "1".into() }).await
    }
}
```

- `AppEvent`：事件标记 trait（blanket impl）
- `EventListener<E>`：监听器 trait，`#[event_listener]` 作用于 impl 块完成注册
- `EventPublisher`：发布器 trait；`DefaultEventPublisher` 为默认实现（`on_missing_trait` 条件注册，可被用户覆盖）；`EventPublisherExt::publish_event` 是便捷方法
- 分派：按事件具体类型 `type_id` 分桶，监听器失败仅记日志，不中断广播
- 弱引用断环：发布器对监听器仅持 `Weak` 而非强引用。监听器常注入 `Arc<dyn EventPublisher>`（对发布器持强引用），若发布器再强引用监听器则两者互持形成引用环：组件永远无法释放，销毁时 `Arc` 计数无法归 1 而失败。`Weak` 断开此环——监听器由组件仓库独立持有，销毁后分派时自动跳过

## 三、组合使用示例

以下示例串联配置、组件、trait 注入、定时任务与启动钩子：

```rust
use simple_starter_core::{anyhow, component, configuration, cron_job, inject, injectable, provider};
use simple_starter_core::{AppCoreUtil, Application};
use std::sync::Arc;

// 1. 配置组件
#[derive(serde::Deserialize)]
#[configuration("database")]
struct DbConfig { url: String }

// 2. trait + 多实现（插件定义接口、用户提供实现）
trait FileParser: simple_starter_core::Injectable {
    fn parse(&self, content: &str) -> anyhow::Result<Vec<String>>;
}

#[component]
struct JsonParser;
#[injectable]
impl FileParser for JsonParser {
    fn parse(&self, content: &str) -> anyhow::Result<Vec<String>> { Ok(vec![content.into()]) }
}

// 3. 函数工厂（第三方类型）+ 参数自动注入
#[provider]
async fn db_factory(cfg: Arc<DbConfig>) -> anyhow::Result<Database> {
    Database::connect(&cfg.url).await
}

// 4. 业务组件：注入具体类型 + trait 全部实现
#[component(init_method = "init")]
struct ParserService {
    #[inject]
    db: Arc<Database>,
    #[inject]
    parsers: Vec<Arc<dyn FileParser>>,
}
impl ParserService {
    async fn init(&self) -> anyhow::Result<()> { Ok(()) }
}

// 5. 定时任务
#[cron_job("0/30 * * * * *")]
async fn cleanup_task() { /* 每 30 秒清理 */ }

fn main() {
    Application::new()
        .add_default_config(toml::toml! {
            [database]
            url = "sqlite://./data.db"
        })
        .add_startup_hook(async {
            let db = AppCoreUtil::get_component::<Database>()?;
            tracing::info!("database ready: {:?}", db);
            Ok(())
        })
        .run();
}
```

## 四、扩展点

| 扩展点 | 机制 | 使用方式 |
|---|---|---|
| **Plugin trait** | 插件三阶段生命周期 + 拓扑排序 | 实现 `Plugin`，`register_plugin` 注册；`dependencies()` 声明插件依赖 |
| **扩展上下文（Extensions）** | AnyMap 类型键存取 | `Application::insert_extension` / `get_extension_mut`，插件间传递协作注册表 |
| **组件扩展（trait 对象注入）** | `Injectable` + `#[injectable]` + 条件注册 | 定义 trait 接口（继承 `Injectable`），插件提供默认实现（`on_missing_trait` 条件注册），用户注册自己的实现自动覆盖 |
| **条件注册** | `ComponentCondition::Custom` | 自定义 `fn(&ConditionContext) -> bool`，注册期评估 |
| **事件系统** | `AppEvent` + `#[event_listener]` + `EventPublisher` | 定义事件类型、实现监听器组件、注入发布器；也可覆盖 `DefaultEventPublisher` |
| **Tokio 运行时** | `set_tokio_runtime_factory` | 自定义运行时构建（如设置全局线程池参数） |
| **日志层** | `add_log_layer_factory` | 追加自定义 tracing layer（如 OpenTelemetry 导出） |
| **后台任务** | `register_task_spawn_factory` | 注册伴随应用生命周期的异步任务，接收 `CancellationToken` 优雅退出 |
| **主循环接管** | `set_main_loop_hook` | GUI 等场景接管主线程，框架后台派发核心任务，用户择机调用 `shutdown()` |

### 典型扩展场景：定义可覆盖的插件接口

```rust
// 插件侧：定义接口 + 默认实现（条件注册）
pub trait CacheService: simple_starter_core::Injectable {
    async fn get(&self, key: &str) -> Option<String>;
}

#[component(condition = simple_starter_core::ComponentCondition::on_missing_trait::<dyn CacheService>())]
pub struct InMemoryCacheService;
#[injectable]
#[async_trait::async_trait]
impl CacheService for InMemoryCacheService {
    async fn get(&self, key: &str) -> Option<String> { None }
}

// 用户侧：注册自己的实现，默认实现自动退位
#[component]
pub struct RedisCacheService;
#[injectable]
#[async_trait::async_trait]
impl CacheService for RedisCacheService {
    async fn get(&self, key: &str) -> Option<String> { redis::get(key).await }
}
```

## 五、启动流程图

```mermaid
graph TD
    Start(["Application::run"]) --> ConfigLoad

    subgraph S_Config ["1. 配置与日志加载"]
        ConfigLoad[加载全局配置] --> LoadBase["加载 application.toml"]
        LoadBase --> CheckProfile{是否存在 Profile?}
        CheckProfile -- 是 --> LoadProfile["加载 application-{profile}.toml"]
        LoadProfile --> MergeConfig["合并配置: 默认 + 基础 + Profile"]
        CheckProfile -- 否 --> MergeConfig
        MergeConfig --> InitTracing[初始化 Tracing 日志系统]
        InitTracing --> SetupLayers[设置日志层 & 文件守卫]
    end

    subgraph S_Runtime ["2. 运行时初始化"]
        SetupLayers --> InitRuntime[初始化 Tokio 运行时]
        InitRuntime --> CheckFactory{是否有自定义工厂?}
        CheckFactory -- 是 --> UseFactory[使用自定义运行时工厂]
        CheckFactory -- 否 --> BuildRuntime[构建 多线程/单线程 运行时]
    end

    subgraph S_Start ["3. 启动阶段"]
        UseFactory --> CallStart["调用 self.start()"]
        BuildRuntime --> CallStart
        CallStart --> PluginSort[按依赖排序插件]
        PluginSort --> PluginAssemble["循环: plugin.assemble() 装配期"]
        PluginAssemble --> CheckComps{是否存在组件?}

        subgraph S_Components ["组件加载流程"]
            CheckComps -- 是 --> CompLoad[加载组件仓库]
            CompLoad --> CompReg[注册并检查名称唯一性]
            CompReg --> CompCond[条件评估与过滤]
            CompCond --> CompTopo[计算依赖拓扑顺序]
            CompTopo --> CompCycle{检测到循环依赖?}
            CompCycle -- 是 --> Error[返回错误]
            CompCycle -- 否 --> CompCreate["循环: processor.create()"]
            CompCreate --> CompInit["循环: processor.init()"]
        end

        CheckComps -- 否 --> PluginCompReady
        CompInit --> PluginCompReady["循环: plugin.components_ready() 组件就绪期"]
        PluginCompReady --> PluginFinalize["循环: plugin.finalize() 收尾期"]
        PluginFinalize --> StartHooks[执行启动钩子 Startup Hooks]
    end

    subgraph S_Execution ["4. 主运行循环"]
        StartHooks --> CheckMainLoop{是否有自定义主循环?}
        CheckMainLoop -- "是 (如 GUI)" --> SpawnCore["后台派发 App 核心管理任务"]
        SpawnCore --> UserLoop[执行用户自定义主循环钩子]
        UserLoop --> UserShutdown["用户手动调用 shutdown()"]
        CheckMainLoop -- "否 (默认)" --> BlockCore["阻塞等待 App 核心任务"]

        subgraph S_CoreTask ["核心任务逻辑"]
            BlockCore --> SchedCreate[创建并启动 Cron 调度器]
            SchedCreate --> TaskSpawn[派发注册的异步任务]
            TaskSpawn --> WaitSignal[等待退出信号 Ctrl+C / SIGTERM]
        end

        WaitSignal --> AutoShutdown[触发自动关闭流程]
    end

    subgraph S_Shutdown ["5. 关闭流程"]
        UserShutdown --> ShutdownStart["执行 shutdown()"]
        AutoShutdown --> ShutdownStart
        ShutdownStart --> CancelToken[取消异步任务 Token]
        CancelToken --> WaitCore[等待核心任务结束]
        WaitCore --> DownHooks[执行关闭钩子 Shutdown Hooks]
        DownHooks --> PluginDown["插件关闭 (逆序)"]
        PluginDown --> CompDown["组件销毁 (逆序)"]
        CompDown --> End([程序退出])
    end

    style Start fill:#f9f,stroke:#333,stroke-width:2px
    style End fill:#f9f,stroke:#333,stroke-width:2px
    style Error fill:#f00,stroke:#333,color:#fff
    style S_Config fill:#e1f5fe,stroke:#01579b
    style S_Components fill:#fff3e0,stroke:#e65100
    style S_Shutdown fill:#ffebee,stroke:#b71c1c
```
