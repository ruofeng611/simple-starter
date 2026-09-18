# simple-starter-macro

`simple-starter-macro` 提供 simple-starter 框架的全部**过程宏**，负责将声明式注解展开为组件注册、依赖注入、生命周期回调注册、路由挂载、安全资源收集等底层代码。

> **注意**：本 crate 是纯过程宏 crate，用户**无需直接依赖**它——核心宏由 `simple-starter-core` 重导出，Web 宏由 `simple-starter-web` 重导出，安全宏由 `simple-starter-security` 重导出。仅在编写插件模块（需要同时使用三个模块的宏）时可能需要直接依赖。

## 一、基本原理

### 1. inventory 编译期静态收集

Rust 没有运行时反射。宏的核心思路是：**把声明式注解展开为静态注册元数据，交给 `inventory` 编译期收集**。

```rust
#[component(name = "userService")]
struct UserService { /* ... */ }
```

展开为（示意）：

```rust
struct UserService { /* ... */ }

// 构造元数据：依赖列表 + 构造闭包 + 生命周期闭包
::simple_starter_core::submit! {
    ::simple_starter_core::ComponentProcessorFactory {
        dependencies: &[],
        trait_dependencies: &[],
        type_dependencies: &[],
        name: "userService",
        condition: None,
        constructor: || {
            let wrapper = ::simple_starter_core::ComponentWrapper::<UserService>::new(
                /* create_fn: 组装字段注入并构造实例 */,
                /* init_fn: 调用 init_method */,
                /* destroy_fn: 调用 destroy_method */,
            );
            Box::new(wrapper)
        }
    }
}
```

应用启动时遍历 inventory 收集到的全部 `ComponentProcessorFactory`，完成注册、条件过滤、拓扑排序与创建。

### 2. 绝对路径引用运行时

宏展开代码通过**绝对路径**引用运行时与兄弟模块（`::simple_starter_core::...`、`::simple_starter_web::...`、`::simple_starter_security::...`），与用户的 `use` 导入无关，任何命名空间下展开都能正确解析。

### 3. 条件惰性闭包

`condition` 参数接受任意表达式，宏将其包进惰性闭包（`Some(|| expr)`）在注册期求值一次，不受 static 初始化上下文限制：

```rust
#[component(condition = simple_starter_core::ComponentCondition::on_missing_trait::<dyn CacheService>())]
pub struct DefaultCacheService;
```

## 二、导出宏用法

### 核心宏（由 simple-starter-core 重导出）

#### 1. `#[component]` —— 结构体组件

标记结构体为组件，纳入生命周期管理。支持参数：

| 参数 | 说明 |
|---|---|
| `name` | 组件名（默认用结构体短名） |
| `init_method` | 初始化方法名（注入完成后立即执行，拓扑序保证依赖组件先完成初始化）。对应签名 `async fn init(&self)`：以共享引用调用，实例所有权仍在仓库，仅能读取/借用自身 |
| `destroy_method` | 销毁方法名（退出时按创建逆序调用）。对应签名 `async fn destroy(self)`：以「有所有权」的实例调用，可消费字段、取出内部资源 |
| `condition` | 注册条件表达式（不满足则不注册） |

```rust
#[component(name = "databaseComponent", init_method = "init", destroy_method = "disconnect")]
struct DatabaseComponent {
    url: String,                       // 非注入字段用 Default::default() 填充
    #[inject] cache: Arc<dyn CacheService>,  // 注入字段见 #[inject]
}

impl DatabaseComponent {
    async fn init(&self) -> anyhow::Result<()> { Ok(()) }
    async fn disconnect(self) -> anyhow::Result<()> { Ok(()) }
}
```

> 注意两个方法拿到组件的形式不同：`init` 以 `&self` 共享引用调用（实例所有权仍在仓库）；`destroy` 以 `self` 所有权调用（实例已从仓库移出，可取出内部资源，如归还连接池、写回文件）。

#### 2. `#[provider]` —— 函数工厂

把函数注册为组件工厂，适用于第三方库类型或需要复杂初始化的对象。函数参数自动按类型注入（规则同 `#[inject]`）；返回类型（自动剥离 `Result` 外层）即组件类型。支持参数：

| 参数 | 说明 |
|---|---|
| `name` | 组件名（默认用类型短名） |
| `destroy_method` | 销毁逻辑（函数路径或闭包表达式） |
| `condition` | 注册条件表达式（不满足则不注册） |
| `primary` | 无值标记，声明该实例为返回类型的**首要实例**（见下） |

```rust
#[provider(destroy_method = db_destroy)]
async fn db_factory(cfg: Arc<DbConfig>) -> Result<DatabaseConnection, DbErr> {
    let db = Database::connect(&cfg.url).await?;
    Ok(db)
}

async fn db_destroy(db: DatabaseConnection) -> anyhow::Result<()> { Ok(()) }
```

**`primary` 标记**：同类型有多个实例时，声明按类型获取（`get_component::<T>()`）时优先返回的那个。同一类型至多一个 primary。仅 `#[provider]` 支持——同类型多实例只能由多个工厂函数产生；`#[component]` / `#[configuration]` 一个类型只有一个实例，无需声明。

```rust
// 两个同类型实例，mainDb 是按类型获取时的首要实例
#[provider(name = "mainDb", primary)]
async fn create_main_db() -> anyhow::Result<Database> { /* ... */ }

#[provider(name = "backupDb")]
async fn create_backup_db() -> anyhow::Result<Database> { /* ... */ }

#[component]
struct UserService {
    #[inject] db: Arc<Database>,               // 优先注入 mainDb（primary）
    #[inject(name = "backupDb")] backup: Arc<Database>, // 按名注入
}
```

**参数注入**：provider 函数参数**默认全部注入**（`#[inject]` 标记可选，仅作声明）；支持按类型与按名两种方式：

| 参数形态 | 行为 |
|---|---|
| `Arc<T>` | 按具体类型注入（primary 优先 → 短名快速路径 → 唯一实例兜底） |
| `Arc<T>` + `#[inject(name = "x")]` | 按组件名精确注入 |
| `Arc<dyn Trait>` | 按 trait 注入唯一实现（多个实现报错） |
| `Arc<dyn Trait>` + `#[inject(name = "x")]` | 按 trait + 组件名注入指定实现 |
| `Vec<Arc<dyn Trait>>` | 收集该 trait 全部实现（带 `#[inject(name = ...)]` 会编译报错） |

```rust
#[provider]
async fn report_service(
    cfg: Arc<DbConfig>,                                        // 按类型注入
    #[inject(name = "backupDb")] backup: Arc<Database>,        // 按名注入具体类型
    #[inject(name = "JsonParser")] json: Arc<dyn FileParser>,  // 按名注入 trait 实现
    all: Vec<Arc<dyn FileParser>>,                             // 收集全部实现
) -> anyhow::Result<ReportService> { /* ... */ }
```

#### 3. `#[configuration]` —— 配置组件

将结构体注册为配置组件：启动时从全局配置（TOML）按 `prefix` 反序列化，要求结构体实现 `serde::Deserialize`。单参数简写：`#[configuration("server.http")]`；完整写法支持 `name` 与 `condition`。

```rust
#[derive(serde::Deserialize)]
#[configuration("database")]
struct DbConfig { url: String }
```

#### 4. `#[inject]` —— 依赖注入标记

作用于组件字段或 provider 参数。支持形式：

| 形式 | 语义 |
|---|---|
| `#[inject]` | 按类型注入（该类型声明了 primary 时优先返回 primary） |
| `#[inject("name")]` / `#[inject(name = "name")]` | 按名称注入 |

配合类型形态：`Arc<T>`（具体类型）、`Arc<dyn Trait>`（trait 唯一实现 / 按名称指定实现）、`Vec<Arc<dyn Trait>>`（全部实现，不可按名）。

```rust
#[component]
struct ParserController {
    #[inject] json: Arc<dyn FileParser>,                 // 唯一实现
    #[inject(name = "CsvFileParser")] csv: Arc<dyn FileParser>, // 指定实现
    #[inject] all: Vec<Arc<dyn FileParser>>,             // 全部实现
}
```

#### 5. `#[injectable]` —— trait 实现注册

作用于 `impl Trait for Type` 块，注册 trait → 实现映射：`trait_type_id` + `impl_type_id` + 类型擦除 accessor（记录 coercion 瞬间的真实 vtable，供 trait 还原）。

```rust
#[injectable]
impl FileParser for JsonParser {
    fn parse(&self, content: &str) -> anyhow::Result<Vec<String>> { /* ... */ }
}
```

#### 6. `#[lifecycle]` —— 容器级周期回调

作用于 `impl ComponentLifecycle for Type` 块，注册容器级批次回调（两个方法均有默认空实现，按需覆写）：

| 方法 | 时机 |
|---|---|
| `after_all_ready(&self, container)` | 全部组件 create + init 完成后，按创建序正序批次执行 |
| `before_destroy(&self, container)` | 全部组件准备销毁前，按创建序逆序批次执行（此时全局缓存未清空、全部组件存活可互相解析） |

签名带容器 `Arc` 引用（与 `AppContext::container()` 返回形态一致），回调内 deref 直接查询任意组件；需传播所有权时显式 clone。

```rust
#[lifecycle]
#[async_trait::async_trait]
impl ComponentLifecycle for DatabaseComponent {
    async fn after_all_ready(
        &self,
        container: &Arc<ComponentContainer>,
    ) -> anyhow::Result<()> {
        let cache = container.get_component::<CacheService>()?;
        Ok(())
    }
}
```

#### 7. `#[cron_job]` —— 声明式定时任务

作用于 `async fn`，用函数名注册任务：

```rust
#[cron_job("*/5 * * * * *")]  // 每 5 秒执行
async fn heartbeat_task() { tracing::info!("心跳检查"); }
```

#### 8. `#[event_listener]` —— 事件监听器

作用于 `impl EventListener<E> for Type` 块，将实现组件注册为该事件类型的监听器：发布器在容器就绪批次（`after_all_ready`）自动收集，事件发布时广播（详见 core README 事件系统）。监听器仅作为回调组件被发布器收集，**不参与 trait 注入**（无需 `#[injectable]`）。

```rust
#[component]
struct LoginListener;
#[event_listener]
#[async_trait::async_trait]
impl EventListener<UserLoginEvent> for LoginListener {
    async fn on_event(&self, event: &UserLoginEvent) -> anyhow::Result<()> { Ok(()) }
}
```

### Web 宏（由 simple-starter-web 重导出）

| 宏 | 作用 | 详见 |
|---|---|---|
| `#[get]` / `#[post]` / `#[put]` / `#[delete]` | 自由函数路由注册（`path` / `state` 参数） | [web README](../simple-starter-web/README.md) |
| `#[rest_controller]` | impl 块级 REST 控制器（基础路径 + 方法批量注册） | [web README](../simple-starter-web/README.md) |
| `#[get_mapping]` / `#[post_mapping]` / `#[put_mapping]` / `#[delete_mapping]` | 控制器方法级路由标记 | [web README](../simple-starter-web/README.md) |
| `#[json_response]` | 将 handler 返回值自动包装为 `axum::Json<T>` | [web README](../simple-starter-web/README.md) |

### 安全宏（由 simple-starter-security 重导出）

| 宏 | 作用 | 详见 |
|---|---|---|
| `#[security]` | 自由函数安全资源注册（配合路由宏） | [security README](../simple-starter-security/README.md) |
| `#[security_controller]` | impl 块级安全模块标记（配合 `#[rest_controller]`） | [security README](../simple-starter-security/README.md) |
| `#[security_resource]` | 方法级资源标记（显式标记才注册） | [security README](../simple-starter-security/README.md) |

## 三、组合使用示例

以下示例串联多个宏实现一个 Controller：

```rust
use simple_starter_core::{component, inject};
use simple_starter_security::{security_controller, security_resource};
use simple_starter_web::{post_mapping, rest_controller, json_response_wrap, JsonResponse};
use simple_starter_web::axum::extract;
use std::sync::Arc;

#[component]
pub struct StudentController {
    #[inject]
    student_service: Arc<StudentService>,
}

#[security_controller]
#[rest_controller("/student")]
impl StudentController {
    #[post_mapping("/add")]
    #[security_resource]
    pub async fn add_student(
        &self,
        extract::Json(student): extract::Json<StudentDto>,
    ) -> JsonResponse {
        json_response_wrap!(function_name = "添加学生", {
            self.student_service.add(student).await?;
            Ok(())
        })
    }
}
```

展开后：`StudentController` 注册为组件并注入 `StudentService`；`#[rest_controller]` 为方法生成 Axum 路由 handler 并注册 `RouteFactory`；`#[security_controller]` + `#[security_resource]` 注册 `ResourceEntry` 安全资源。
