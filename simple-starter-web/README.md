# `simple-starter-web` 使用教程

`simple-starter-web` 是一个基于 **axum** 构建的轻量级 Web 服务辅助库，它通过 **过程宏 + 自动注册机制** 简化路由定义、配置加载与服务启动流程。你无需手动拼接 Router 或管理服务器生命周期，只需关注业务逻辑。

---

## 一、核心特性（Web 部分）

1. **声明式路由宏**  
   使用 `#[get("/path")]`、`#[post("/path")]` 等属性宏直接标注 handler 函数，自动注册到主路由。

2. **自动 JSON 响应包装**  
   用 `#[json_response]` 标记函数，返回值自动包裹为 `Json<T>`，无需手动写 `Json(...)`。

3. **配置驱动**  
   通过 `application.toml` 配置端口、绑定地址、base path、日志等，支持默认值回退。

4. **零样板启动**  
   注册 `WebPlugin` 后，应用启动时自动合并所有路由、启动服务器，并支持优雅关闭。

5. **编译期路由收集**  
   基于 `inventory` crate，在编译期将所有路由工厂收集到全局静态集合，启动时一次性合并。

6. **统一响应体包装宏 (`json_response_wrap!`)**
     提供一个灵活的声明式宏，用于在 handler 内部构建符合统一格式（`JsonResponse`）的响应。它能自动处理 `Result` 类型，将成功数据或错误信息封装进标准结构，并支持自定义状态码、消息和功能名等元信息。

---

## 二、使用步骤

### Step 1：添加依赖

在 `Cargo.toml` 中添加：

```toml
[dependencies]
simple-starter-core = { path = "../simple-starter/simple-starter-core" }
simple-starter-macro = { path = "../simple-starter/simple-starter-macro" }
simple-starter-web  = { path = "../simple-starter/simple-starter-web" }
serde = { version = "1.0.228", features = ["derive"] }
```

---

### Step 2：编写 Handler 函数

使用路由宏和 `#[json_response]` 简化代码：

```rust
use simple_starter_macro::{get, json_response};
use simple_starter_web::axum;

#[get("/hello")]
#[json_response]
async fn hello() -> String {
   "Hello, world!".to_string()
}

#[get("/user/{id}")]
#[json_response]
async fn get_user(axum::extract::Path(id): axum::extract::Path<u32>) -> User {
   User {
      id,
      name: "Alice".into(),
   }
}

#[derive(serde::Serialize)]
struct User {
   id: u32,
   name: String,
}
```

- `#[get("/path")]`：自动注册 GET 路由。
- `#[json_response]`：要求函数是 `async` 且有返回类型，自动转为 `Json<T>`。
- 路径参数通过 `axum::extract::Path` 提取（标准 axum 用法）。

##### Step 2.5：使用 `json_response_wrap!` 宏进行细粒度控制（可选）

虽然 `#[json_response]` 属性宏适用于简单场景，但当您需要在 handler 内部处理复杂的业务逻辑、可能产生多种错误或需要自定义响应码时，`json_response_wrap!` 声明宏是更好的选择。

```rust
use simple_starter_macro::{get, json_response};
use simple_starter_web::{JsonResponse, axum, json_response_wrap};

#[get("/user-wrap/{id}")]
#[json_response]
async fn get_user_with_wrap(axum::extract::Path(id): axum::extract::Path<u32>) -> JsonResponse {
   // 使用 json_response_wrap! 宏包裹核心逻辑
   json_response_wrap!(code = 200, message = "User fetched successfully", function_name = "get_user_with_wrap", {
            if id == 0 {
                // 示例：一个可能失败的操作
                return Err(SimpleAppWebError::new(400, "Invalid user ID"));
            }
            Ok(User {
                id,
                name: "Alice".into(),
            })
        }
    )
}
```

**宏参数说明 (`json_response_wrap!`)**:
该宏非常灵活，支持多种参数组合：
- **无参数**: `json_response_wrap!({ ... })`
   - 默认 `code=200`, `message="操作成功"`.
- **仅 `code`**: `json_response_wrap!(code=404, { ... })`
- **仅 `message`**: `json_response_wrap!(message="Not Found", { ... })`
- **仅 `function_name`**: `json_response_wrap!(function_name="my_func", { ... })`
- **任意组合**: 如 `code` + `message`, `message` + `function_name` 等。
- **全参数**: `json_response_wrap!(code=..., message=..., function_name=..., { ... })`

宏内部的代码块 `{ ... }` **必须**返回一个 `Result<T, SimpleAppWebError>`。如果返回 `Ok(T)`，`T` 会被序列化并放入响应体的 `data` 字段；如果返回 `Err(e)`，则会使用错误 `e` 的 `code` 和 `message` 来构造响应。

---

### Step 3：配置 `application.toml`

在 `./resources/application.toml` 中添加 Web 配置：

```toml
[web]
port = 3000
binding = "127.0.0.1"
base_path = "/api"           # 可选，所有路由前加 /api
log_include_headers = false
worker_thread_num = 4        # 可选
worker_thread_name = "my-web-worker"
```

> 若不提供，将使用默认值：`port=8080`, `binding="0.0.0.0"` 等，无 `base_path`。

---

### Step 4：启动应用

在 `main.rs` 中注册 `WebPlugin` 并运行：

```rust
use simple_starter_core::Application;
use simple_starter_web::WebPlugin;

fn main() {
   Application::new()
           .register_plugin(WebPlugin::new())
           .add_startup_hook(|| println!("Server ready!"))
           .run();
}
```

- `WebPlugin::new()` 会自动扫描所有通过宏注册的路由。
- `.run()` 会加载配置、启动服务器、监听退出信号并优雅关闭。

---

## 三、简单实现原理

1. **路由自动注册**  
   每个 `#[get(...)]` 宏会展开为：
    - 保留原始 handler 函数；
    - 调用 `inventory::submit!(RouteFactory { router: || ... })`。

   `WebPlugin::new()` 在初始化时遍历 `inventory::iter::<RouteFactory>`，调用每个 `router()` 函数并 `merge` 到主 Router。

2. **JSON 自动包装**  
   `#[json_response]` 将：
   ```rust
   async fn f() -> T { ... }
   ```
   转换为：
   ```rust
   async fn f() -> Json<T> {
       let __result = { ... }.await;
       Json(__result)
   }
   ```

3. **配置加载**  
   `WebPlugin::init()` 从全局配置中提取 `web` 节点，反序列化为 `WebConfig` 结构体。

4. **服务器启动**  
   在独立线程中创建多线程 Tokio Runtime，运行 `axum::serve`，并通过 `oneshot` 通道实现优雅关闭。
