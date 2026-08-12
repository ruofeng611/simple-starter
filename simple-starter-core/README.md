# simple-starter-core

`simple-starter-core` 提供 simple-starter 框架的运行时核心：配置管理、组件模型、依赖注入、插件系统、任务调度与生命周期管理。

## Trait Object 注入运行时架构

`#[injectable]` 注册 trait 实现，`#[inject]` 声明 trait 依赖，运行时通过 `TypeId` 直接匹配，无需字符串中转。

### 核心类型

| 类型 | 说明 |
|---|---|
| `Injectable` | 可注入 trait 的 supertrait：`Any + Send + Sync` 的 blanket impl，所有 trait 对象经它统一擦除 |
| `TraitImplRegistration` | inventory 收集的 trait 实现注册：`trait_type_id` / `impl_type_id` / `accessor` |
| `ComponentProcessorFactory.trait_dependencies` | 组件的 trait 依赖列表 `&'static [fn() -> TypeId]`，编译期生成函数指针 |
| `ComponentKey` | `(TypeId, 组件名)`，仓库与缓存的统一键 |

### 启动期流程（component_loader）

1. **构建 trait 实现索引**（`build_trait_impl_index`）：遍历 inventory 中所有 `TraitImplRegistration`，
   按 `trait_type_id` 分组为局部 `HashMap`——仅启动期使用，不占用全局状态。
2. **拓扑排序**（`resolve_component_order`）：组件名依赖（`dependencies`）直接建边；
   trait 依赖（`trait_dependencies`）通过 TypeId 查索引，展开为**所有**实现组件的实例名建边，
   保证实现组件先于依赖者创建。
3. **填充 trait 对象缓存**（`populate_trait_obj_cache`）：组件 create 后，按 `impl_type_id` 找到匹配的注册，
   调用 `accessor` 将 `Arc<ConcreteType>` 擦除为 `Arc<dyn Injectable>`，
   写入 `TRAIT_OBJ_CACHE`（键：`(trait_type_id, 实例名)`），并把实例名追加到 `INSTANCE_NAMES_BY_TRAIT`。

### 运行时获取 API（AppCoreUtil）

| API | 说明 |
|---|---|
| `get_component_by_trait::<Trait>()` | 按 trait 获取唯一实现；多个实例报 `AmbiguousTraitImpl` |
| `get_component_by_trait_and_name::<Trait>(name)` | 按 trait + 实例名获取指定实现 |
| `get_components_by_trait::<Trait>()` | 收集 trait 的全部实现，返回 `Vec<Arc<dyn Trait>>` |

**防死锁设计**：以上 API 只读 `INSTANCE_NAMES_BY_TRAIT` 与 `TRAIT_OBJ_CACHE`，不扫描 `COMPONENT_REPOSITORY`。
否则在 create 阶段（持有 `get_mut` 写锁）遍历仓库会阻塞在同一 shard 上造成死锁。

### 全局状态

| 状态 | 说明 |
|---|---|
| `TRAIT_OBJ_CACHE` | `DashMap<(TypeId, String), Arc<dyn Injectable>>`，trait 对象缓存 |
| `INSTANCE_NAMES_BY_TRAIT` | `DashMap<TypeId, Vec<String>>`，trait 下所有实例名 |

> 已移除：`TRAIT_NAME_TO_TYPE_ID`、`TRAIT_IMPL_INDEX`——字符串索引被 TypeId 直接匹配替代；
> trait 实现索引改为启动期局部变量，随 `resolve_component_order` 使用完毕后释放。
