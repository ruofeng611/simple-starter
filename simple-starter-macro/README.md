# simple-starter-macro

`simple-starter-macro` 提供 simple-starter 框架的全部过程宏，负责将声明式注解展开为组件注册、依赖注入、路由挂载等底层代码。

## 宏列表

| 宏                  | 作用                                         | 示例                                                 |
|:-------------------|:-------------------------------------------|:---------------------------------------------------|
| `#[component]`     | 标记结构体为组件，纳入生命周期管理。                        | `#[component(name = "auth", init_method = "start")]` |
| `#[provider]`      | 将函数标记为组件工厂，用于创建复杂对象。                      | `#[provider] async fn create_db() -> Database`      |
| `#[configuration]` | 将结构体标记为配置组件，从全局配置中反序列化。                  | `#[configuration("constant")]`                      |
| `#[inject]`        | 标记字段/参数需要注入依赖。                             | `#[inject] db: Arc<Database>`                       |
| `#[injectable]`    | 注册 trait 与实现类的映射关系。                        | `#[injectable] impl FileParser for JsonParser`      |
| `#[cron_job]`      | 注册定时任务。                                     | `#[cron_job("*/5 * * * * *")]`                      |
| `#[get]/#[post]...` | 注册 HTTP 路由。                                | `#[get("/user")]`                                   |
| `#[json_response]` | 转换返回值为 Json 格式。                            | `async fn handler() -> JsonResponse`                |
| `#[rest_controller]` | 将 impl 块中的方法批量注册为 HTTP 路由。                 | `#[rest_controller] impl TestController`            |

## Trait Object 注入

`#[injectable]` + `#[inject]` 共同实现 `Arc<dyn Trait>` 形式的依赖注入。

### 1. 注册 trait 实现（#[injectable]）

`#[injectable]` 作用于 `impl Trait for Type` 块，展开为 `TraitImplRegistration` 并提交到 `inventory`：

- `trait_type_id`: `TypeId::of::<dyn Trait>()`
- `impl_type_id`: `TypeId::of::<ConcreteType>()`
- `accessor`: 类型擦除转换函数 `Arc<ConcreteType> → Arc<dyn Injectable>`

```rust
#[injectable]
impl FileParser for JsonParser {
    fn parse(&self, content: &str) -> anyhow::Result<Vec<String>> {
        // ...
    }
}
```

### 2. 注入 trait 依赖（#[inject]）

`#[component]` 的字段与 `#[provider]` 的参数支持三类 trait 依赖声明，
展开为不同的依赖标记与运行时获取调用：

| 声明形式 | 依赖标记（拓扑排序） | 运行时获取调用 |
|---|---|---|
| `Vec<Arc<dyn Trait>>` | trait TypeId，依赖全部实现 | `get_components_by_trait::<dyn Trait>()` |
| `#[inject] Arc<dyn Trait>`（无名字） | trait TypeId，依赖全部实现 | `get_component_by_trait::<dyn Trait>()` |
| `#[inject(name = "X")] Arc<dyn Trait>` | 组件名 `"X"`，只依赖单个组件 | `get_component_by_trait_and_name::<dyn Trait>("X")` |

```rust
#[component]
struct FileParserController {
    // 按 trait 获取唯一实现（多个实现时报错）
    #[inject]
    json_parser: Arc<dyn FileParser>,

    // 按名称获取指定实现
    #[inject(name = "CsvFileParser")]
    csv_parser: Arc<dyn FileParser>,

    // 收集全部实现
    #[inject]
    all_parsers: Vec<Arc<dyn FileParser>>,
}
```

### 3. TypeId 直接匹配（无字符串桥接）

trait 依赖通过编译期生成的函数指针 `fn() -> TypeId` 在运行时直接返回 `TypeId::of::<dyn Trait>()`，
与 `#[injectable]` 注册的 `trait_type_id` 精确匹配。带来的优势：

- **跨 crate 同名 trait 不冲突**：TypeId 全局唯一，不存在字符串短名碰撞
- **`use` 短名导入不受影响**：宏透传用户书写的类型（含完整路径），运行时 `TypeId` 与路径写法无关
- **依赖边精确**：带名字的 trait 注入只对目标组件建依赖边，不牵连该 trait 的其他实现

## rest_controller 宏

### 使用示例如下

```rust
use crate::dto::student_dto::StudentDto;
use crate::service::student_service::StudentService;
use simple_starter_macro::{component, post_mapping, rest_controller};
use simple_starter_web::axum::extract;
use simple_starter_web::{json_response_wrap, JsonResponse};
use std::sync::Arc;

#[component]
pub struct TestController {
    #[inject]
    student_service: Arc<StudentService>,
}

#[rest_controller]
impl TestController {
    #[post_mapping("/student/add/{id}")]
    pub async fn get_student_name(
        &self,
        extract::Path(id): extract::Path<i64>,
        extract::Json(student): extract::Json<StudentDto>,
    ) -> JsonResponse {
        json_response_wrap!(function_name = "根据学生id获取学生姓名", {
            println!("id: {}", id);
            println!("student: {:?}", student);
            println!("find_student_name: {:?}", self.student_service.get_student_name(id).await);
            Ok(())
        })
    }
}
```
