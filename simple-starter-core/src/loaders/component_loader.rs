use crate::core::app_component::{
    ComponentProcessor, ComponentProcessorFactory, PrimaryRegistration, TraitImplRegistration,
};
use crate::core::app_types::ComponentKey;
use crate::loaders::component_condition::{ComponentCondition, ConditionContext};
use crate::global_state::{
    COMPONENT_REPOSITORY, PRIMARY_BY_TYPE, TRAIT_INSTANCE_NAMES, TRAIT_OBJ_CACHE,
    TYPE_INSTANCE_NAMES,
};
use crate::utils::app_inner_util::{
    build_component_indexes, build_impl_registration_index, build_trait_impl_index,
};
use anyhow::{Context, anyhow};
use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// 全局组件启动顺序队列
///
/// 用于记录组件的创建顺序，以便在销毁时按逆序进行。
/// 受 Mutex 保护，并发读写。
pub(crate) static COMPONENT_ORDER: Mutex<Vec<ComponentKey>> = Mutex::new(Vec::new());

/// 加载并初始化所有组件
///
/// 包含三个步骤：
/// 1. 注册阶段：扫描 inventory 工厂、校验名称唯一、条件过滤，构建 DFS 索引。
/// 2. 创建阶段：DFS 深度优先创建组件，按需展开四类依赖（名称/trait/类型/primary）。
/// 3. 初始化阶段：按创建顺序统一执行 init。
pub(crate) async fn component_repository_load() -> anyhow::Result<()> {
    let plan = register_components()?;
    run_creation(plan).await?;
    run_init().await?;
    Ok(())
}

// =============================================================================
// 注册阶段
// =============================================================================

/// 组件加载计划（注册阶段产物，仅启动期局部使用，加载完成即释放）
struct LoadPlan {
    /// 组件名 → 创建计划条目
    entries: HashMap<String, PlanEntry>,
    /// trait 实现索引：trait_type_id → 所有实现的具体类型（trait 依赖展开用）
    trait_impl_index: HashMap<TypeId, Vec<TypeId>>,
    /// 具体类型 → 实例名列表 索引（trait 依赖展开使用，避免 DFS 期间扫描仓库）
    type_instance_index: HashMap<TypeId, Vec<String>>,
    /// 实现类型 → 匹配的 trait 实现注册列表（一对多，populate 填充缓存用）
    impl_registration_index: HashMap<TypeId, Vec<&'static TraitImplRegistration>>,
}

/// 单个组件的创建计划（依赖声明来自编译期工厂元数据）
struct PlanEntry {
    /// 具体类型依赖（组件名列表）
    dependencies: Vec<&'static str>,
    /// trait 依赖的 TypeId 列表（DFS 时展开为全部实现组件）
    trait_dependencies: Vec<TypeId>,
    /// 具体类型依赖的 TypeId 列表（DFS 时展开为该类型全部实例；
    /// 无名称注入 `Arc<T>` 时使用，组件可能自定义名称）
    type_dependencies: Vec<TypeId>,
    /// primary 依赖的 TypeId 列表（`#[inject_primary]` 生成；
    /// DFS 时仅建边到该类型的 primary 实例，不强制创建同类型其他实例）
    primary_dependencies: Vec<TypeId>,
    /// 条件声明：None 无条件注册；Some 为注册期已求值的条件
    condition: Option<ComponentCondition>,
}

/// 注册组件阶段
///
/// 遍历 inventory 收集的工厂，校验名称唯一性，存入仓库，
/// 构建 DFS 创建所需的索引并返回加载计划。
fn register_components() -> anyhow::Result<LoadPlan> {
    let mut entries: HashMap<String, PlanEntry> = HashMap::new();
    let mut registered_names: HashSet<String> = HashSet::new();

    // 1. 遍历所有注册的工厂
    for factory in inventory::iter::<ComponentProcessorFactory> {
        let name = factory.name;

        // 2. 严格的名称唯一性检查（组件名即全局唯一 key，跨类型重名亦被拒绝）
        if registered_names.contains(name) {
            return Err(anyhow::anyhow!(
                "Duplicate component name detected: '{}'. Component names must be globally unique.",
                name
            ));
        }
        registered_names.insert(name.to_string());

        // 3. 构造组件 Wrapper (此时 inner 为 None)
        let processor: Box<dyn ComponentProcessor> = (factory.constructor)();

        COMPONENT_REPOSITORY.insert(name.to_string(), processor);

        // 4. 记录创建计划（名称依赖 + trait 依赖 + 类型依赖 + primary 依赖）
        let trait_type_ids: Vec<TypeId> = factory.trait_dependencies.to_vec();
        let type_type_ids: Vec<TypeId> = factory.type_dependencies.to_vec();
        let primary_type_ids: Vec<TypeId> = factory.primary_dependencies.to_vec();
        entries.insert(
            name.to_string(),
            PlanEntry {
                dependencies: Vec::from(factory.dependencies),
                trait_dependencies: trait_type_ids,
                type_dependencies: type_type_ids,
                primary_dependencies: primary_type_ids,
                condition: factory.condition.map(|get_condition| get_condition()),
            },
        );
    }

    // 5. 注册期条件过滤（不满足的组件从仓库与计划中移除，不参与 DFS 创建）
    filter_components_by_condition(&mut entries)?;

    // 6. 构建 primary 索引（条件过滤后，校验 primary 名字对应组件存在、同类型唯一）
    build_primary_index()?;

    // 7. 构建 DFS 所需索引（启动期局部数据，加载完成后即释放）
    let trait_impl_index = build_trait_impl_index();
    let impl_registration_index = build_impl_registration_index();
    // 组件名快照已由 entries 的 key 集表达（条件过滤同步移除仓库与计划，两者等价）
    let (_, type_instance_index) = build_component_indexes();

    Ok(LoadPlan {
        entries,
        trait_impl_index,
        type_instance_index,
        impl_registration_index,
    })
}

/// 注册期条件过滤
///
/// 两阶段语义：inventory 工厂全量登记后统一评估，不满足者从仓库与创建
/// 计划中移除，使其不参与后续 DFS 创建。条件仅依赖"注册信息 + 全局配置"，
/// 评估结果与组件创建顺序无关（对齐 Spring 的 bean definition 期条件评估语义）。
///
/// 单轮评估 + 全量注册快照：评估开始时的快照在整个评估过程中保持不变，
/// 链式互斥条件（A 条件是 B 不存在、B 条件是 A 不存在）两者都注册，
/// 不做不动点迭代，语义可预测优先。
///
/// 仅 inventory 组件携带条件声明。
fn filter_components_by_condition(entries: &mut HashMap<String, PlanEntry>) -> anyhow::Result<()> {
    // 快速路径：无任何条件声明
    if !entries.values().any(|e| e.condition.is_some()) {
        return Ok(());
    }

    // 1. 构建注册全量快照（单轮评估的固定上下文）
    let ctx = ConditionContext::snapshot();

    // 2. 单轮评估，收集不满足者
    let mut to_remove: Vec<String> = Vec::new();
    for (name, entry) in entries.iter() {
        if let Some(condition) = &entry.condition
            && !condition.evaluate(&ctx, name)
        {
            to_remove.push(name.clone());
        }
    }

    // 3. 统一移除：仓库 + 创建计划（DFS 索引在过滤后构建，天然不含被移除组件）
    for name in &to_remove {
        COMPONENT_REPOSITORY.remove(name);
        entries.remove(name);
        tracing::debug!("Component '{}' skipped: condition not satisfied", name);
    }

    Ok(())
}

/// 构建 primary（首要实例）索引
///
/// 在条件过滤后执行：遍历 inventory `PrimaryRegistration`，
/// 校验声明存在性与唯一性（对齐启动期全量验证语义）：
/// - primary 名字必须对应已注册组件；被条件移除 → fail-fast
/// - 同一具体类型只允许一个 primary
fn build_primary_index() -> anyhow::Result<()> {
    for reg in inventory::iter::<PrimaryRegistration> {
        let type_id = reg.type_id;

        // 校验：primary 指向的组件必须已注册（条件过滤后仍存在）
        if !COMPONENT_REPOSITORY.contains_key(reg.name) {
            return Err(anyhow!(
                "Primary instance '{}' is not registered. #[primary] name must match a registered provider component name.",
                reg.name
            ));
        }

        // 校验：同类型 primary 唯一
        if let Some(existing) = PRIMARY_BY_TYPE.insert(type_id, reg.name.to_string()) {
            return Err(anyhow!(
                "Duplicate primary instance for the same type: '{}' and '{}'. Only one primary instance per concrete type is allowed.",
                existing,
                reg.name
            ));
        }
        tracing::debug!(
            "Primary instance registered: '{}' for TypeId={:?}",
            reg.name,
            type_id
        );
    }

    Ok(())
}

// =============================================================================
// 创建阶段（DFS 深度优先）
// =============================================================================

/// DFS 创建阶段的运行时上下文（仅启动期局部使用，加载完成即释放）
struct DfsCreation {
    plan: LoadPlan,
    /// 组件名 → 三色创建状态
    states: HashMap<String, CreateState>,
}

/// 三色创建状态（DFS 环检测与去重）
///
/// "白色"（已注册未创建）状态由 `states` 表中键不存在隐式表示，无需显式变体。
enum CreateState {
    /// 灰：正在 DFS 栈中（再次访问 = 循环依赖）
    Creating,
    /// 黑：已创建完成（再次访问 = 缓存命中，直接跳过）
    Created,
}

/// 阶段一：DFS 深度优先创建组件
///
/// 创建顺序由"依赖先于依赖者"的 DFS 序决定，与拓扑排序等价；
/// 每个组件创建完成后立即填充 trait object 缓存并记录创建顺序。
async fn run_creation(plan: LoadPlan) -> anyhow::Result<()> {
    let mut dfs = DfsCreation {
        plan,
        states: HashMap::new(),
    };

    // 1. 根序列：创建计划全量组件名（entries 的 key 集即过滤后仓库全量组件）
    let roots: Vec<String> = dfs.plan.entries.keys().cloned().collect();

    // 2. 对每个根执行 DFS 创建
    let mut stack = Vec::new();
    for name in &roots {
        dfs.dfs_create(name, &mut stack).await?;
    }

    Ok(())
}

impl DfsCreation {
    /// 按组件名深度优先创建（组件名全局唯一，仓库 key 即组件名）
    async fn dfs_create(&mut self, name: &str, stack: &mut Vec<String>) -> anyhow::Result<()> {
        // 1. 三色状态判断
        match self.states.get(name) {
            Some(CreateState::Created) => return Ok(()), // 黑：已创建，菱形依赖安全
            Some(CreateState::Creating) => {
                // 灰：再次访问 = 循环依赖，栈即为依赖链
                return Err(anyhow!(
                    "Circular dependency detected in components: [{} -> {}]",
                    stack.join(" -> "),
                    name
                ));
            }
            _ => {}
        }

        // 2. 置灰并压栈
        self.states
            .insert(name.to_string(), CreateState::Creating);
        stack.push(name.to_string());

        // 3. 解析依赖清单（先 clone 出四类依赖，避免与递归调用的可变借用冲突）
        let (dependencies, trait_dependencies, type_dependencies, primary_dependencies) = {
            let entry = self
                .plan
                .entries
                .get(name)
                .ok_or_else(|| anyhow!("Component '{}' has no creation plan", name))?;
            (
                entry.dependencies.clone(),
                entry.trait_dependencies.clone(),
                entry.type_dependencies.clone(),
                entry.primary_dependencies.clone(),
            )
        };

        // 4. 名称依赖：按组件名递归创建（依赖未注册时在此 fail-fast）
        for dep_name in &dependencies {
            self.resolve_key(dep_name)?;
            Box::pin(self.dfs_create(dep_name, stack)).await?;
        }

        // 5. trait 依赖：展开为全部实现组件的全部实例
        for trait_type_id in &trait_dependencies {
            let impls = self
                .plan
                .trait_impl_index
                .get(trait_type_id)
                .cloned()
                .ok_or_else(|| {
                    anyhow!(
                        "Component '{}' depends on a trait (TypeId={:?}) that has no registered implementations",
                        name,
                        trait_type_id
                    )
                })?;

            let mut resolved_any = false;
            for impl_type_id in impls.iter() {
                let instance_names = self
                    .plan
                    .type_instance_index
                    .get(impl_type_id)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!(
                            "Component '{}' depends on a trait (TypeId={:?}), but its implementation type {:?} is not registered",
                            name,
                            trait_type_id,
                            impl_type_id
                        )
                    })?;
                for impl_name in &instance_names {
                    Box::pin(self.dfs_create(impl_name, stack)).await?;
                    resolved_any = true;
                }
            }

            if !resolved_any {
                return Err(anyhow!(
                    "Component '{}' depends on a trait (TypeId={:?}) that has no registered component instances",
                    name,
                    trait_type_id
                ));
            }
        }

        // 6. 类型依赖：展开为该类型的所有实例
        for type_id in &type_dependencies {
            let instance_names = self
                .plan
                .type_instance_index
                .get(type_id)
                .cloned()
                .ok_or_else(|| {
                    anyhow!(
                        "Component '{}' depends on type (TypeId={:?}) that has no registered component instances",
                        name,
                        type_id
                    )
                })?;
            for impl_name in &instance_names {
                Box::pin(self.dfs_create(impl_name, stack)).await?;
            }
        }

        // 7. primary 依赖：仅建边到该类型的 primary 实例，
        //    不强制创建同类型其他实例（留给用户按需获取）
        for type_id in &primary_dependencies {
            let primary_name = PRIMARY_BY_TYPE.get(type_id).ok_or_else(|| {
                anyhow!(
                    "Component '{}' depends on a primary instance (TypeId={:?}) that is not registered; a #[primary] must be declared on one of the type's providers",
                    name,
                    type_id
                )
            })?;
            // 注册期 build_primary_index 已校验 primary 名对应的组件存在，此处直接递归创建
            Box::pin(self.dfs_create(&primary_name.value().clone(), stack)).await?;
        }

        // 8. 创建自身：先从仓库移除再执行 create，避免写锁跨 await
        //    与 create 回调读取仓库时同 shard 死锁
        let key = self.resolve_key(name)?;
        let (_, mut processor) = COMPONENT_REPOSITORY
            .remove(&key)
            .ok_or_else(|| anyhow!("Component '{}' not found in repository", name))?;

        let create_result = processor
            .create()
            .await
            .with_context(|| format!("Failed to create component: {}", name));
        COMPONENT_REPOSITORY.insert(key.clone(), processor);
        create_result?;

        tracing::debug!("Component created: {}", name);

        // 9. 创建后立即缓存该组件的 trait object，
        //    确保后续组件在 create 阶段即可通过 get_component_by_trait 获取依赖
        populate_trait_obj_cache(&key, &self.plan.impl_registration_index)?;

        // 10. 记录创建顺序（销毁时逆序使用）
        {
            let mut guard = COMPONENT_ORDER
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock COMPONENT_ORDER (Poisoned)"))?;
            guard.push(key);
        }

        // 11. 置黑并出栈
        self.states.insert(name.to_string(), CreateState::Created);
        stack.pop();
        Ok(())
    }

    /// 依赖名校验（依赖未注册时 fail-fast）；仓库 key 即组件名，校验通过后直接使用
    fn resolve_key(&self, name: &str) -> anyhow::Result<String> {
        if self.plan.entries.contains_key(name) {
            Ok(name.to_string())
        } else {
            Err(anyhow!(
                "Component depends on '{}', but '{}' is not registered.",
                name,
                name
            ))
        }
    }
}

// =============================================================================
// 初始化阶段
// =============================================================================

/// 阶段二：按创建顺序统一初始化
///
/// 与 create 相同，采用 temporarily remove 模式执行 init，
/// 避免写锁跨 await 与 init 回调中读取仓库时同 shard 死锁。
async fn run_init() -> anyhow::Result<()> {
    // Clone 出排序好的 Key 列表，避免持有锁进行 await
    let sorted_keys: Vec<ComponentKey> = {
        let guard = COMPONENT_ORDER
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock COMPONENT_ORDER (Poisoned)"))?;
        guard.clone()
    };

    for key in &sorted_keys {
        let (_, mut processor) = COMPONENT_REPOSITORY
            .remove(key)
            .ok_or_else(|| anyhow!("Component '{}' not found in repository", key))?;

        let init_result = processor
            .init()
            .await
            .with_context(|| format!("Failed to init component: {}", key));
        COMPONENT_REPOSITORY.insert(key.clone(), processor);
        init_result?;

        tracing::debug!("Component initialized: {}", key);
    }

    Ok(())
}

// =============================================================================
// trait object 缓存与销毁
// =============================================================================

/// 为指定组件填充 trait object 缓存与实例名索引
///
/// 通过实现类型注册索引（`impl_registration_index`，启动期构建的局部快照）
/// 按组件具体类型直接查询匹配的注册项（一对多：一个组件类型可注册多个 trait 实现），
/// 通过 accessor 将 `Arc<ConcreteType>` 转换为 `Arc<dyn Injectable>`，
/// 以 **(trait_type_id, 组件实例名)** 为 key 存入 `TRAIT_OBJ_CACHE`。
///
/// 同步填充两个运行时实例名索引：
/// - `TYPE_INSTANCE_NAMES`：类型维度（每个组件 create 后必填）
/// - `TRAIT_INSTANCE_NAMES`：trait 维度（accessor 命中时填充）
///
/// 使用组件实例名作为 cache key（而非 TraitImplRegistration 名称），
/// 确保同一具体类型的多个实例（如通过 provider 创建的同类型不同名称组件）
/// 各自拥有独立的 cache 条目。
fn populate_trait_obj_cache(
    key: &ComponentKey,
    impl_registration_index: &HashMap<TypeId, Vec<&'static TraitImplRegistration>>,
) -> anyhow::Result<()> {
    let entry = COMPONENT_REPOSITORY.get(key).ok_or_else(|| {
        anyhow::anyhow!("Component '{}' not found in repository", key)
    })?;
    let processor = entry.value();

    // 获取类型擦除的 Arc
    let arc_any = match processor.get_inner_arc_any() {
        Some(a) => a,
        None => return Err(anyhow::anyhow!("Component '{}' not created", key)), // 还未 create，不可能发生，直接报错
    };

    // 组件具体类型取自实例本身（`ComponentProcessor::type_id()`）
    let component_type_id = ComponentProcessor::type_id(&**processor);
    let component_instance_name = key;

    // 1. 填充类型维度索引：具体类型 → 全部实例名
    {
        let mut names_entry = TYPE_INSTANCE_NAMES.entry(component_type_id).or_default();
        if !names_entry.contains(component_instance_name) {
            names_entry.push(component_instance_name.clone());
        }
    }

    // 2. 按实现类型索引查询匹配的 trait 实现注册，填充 trait 缓存与 trait 维度索引
    if let Some(regs) = impl_registration_index.get(&component_type_id) {
        for reg in regs {
            if let Some(entry) = (reg.accessor)(arc_any.clone()) {
                // cache key: (trait_type_id, 组件实例名)
                let cache_key = (reg.trait_type_id, component_instance_name.clone());
                TRAIT_OBJ_CACHE.insert(cache_key, entry);
                {
                    let mut names_entry = TRAIT_INSTANCE_NAMES.entry(reg.trait_type_id).or_default();
                    if !names_entry.contains(component_instance_name) {
                        names_entry.push(component_instance_name.clone());
                    }
                }
                tracing::debug!(
                    "Registered trait object: instance '{}' as trait TypeId={:?}",
                    component_instance_name,
                    reg.trait_type_id
                );
            }
        }
    }
    Ok(())
}

/// 关闭并销毁所有组件
///
/// 销毁顺序为创建顺序的逆序（后创建者先销毁，保证依赖方向安全）。
pub(crate) async fn shutdown_components() -> anyhow::Result<()> {
    // 1. 获取并清空全局顺序列表 (防止多次 shutdown 重复执行)
    let sorted_keys: Vec<ComponentKey> = {
        let mut guard = COMPONENT_ORDER
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock COMPONENT_ORDER (Poisoned)"))?;
        std::mem::take(&mut *guard)
    };

    if sorted_keys.is_empty() {
        tracing::warn!("No components to shutdown or COMPONENT_ORDER is already empty.");
        return Ok(());
    }

    // 2. 销毁前先清空 trait object 缓存与实例名索引，释放对组件实例的额外引用，
    //    确保后续 destroy() 中 Arc::try_unwrap 的 refcount 为 1。
    TRAIT_OBJ_CACHE.clear();
    TYPE_INSTANCE_NAMES.clear();
    TRAIT_INSTANCE_NAMES.clear();
    PRIMARY_BY_TYPE.clear();

    // 3. 逆序遍历，从仓库移除所有权并调用 destroy
    for key in sorted_keys.iter().rev() {
        if let Some((_, mut processor)) = COMPONENT_REPOSITORY.remove(key) {
            // 销毁失败只记录错误，不中断流程，保证其他组件有机会销毁
            if let Err(e) = processor.destroy().await {
                tracing::error!("Error destroying component '{}': {:?}", key, e);
            } else {
                tracing::debug!("Component '{}' destroyed successfully.", key);
            }
        }
    }

    Ok(())
}
