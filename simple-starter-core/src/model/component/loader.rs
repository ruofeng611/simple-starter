//! # 组件装配流程（加载与销毁）
//!
//! `ComponentContainer` 生命周期方法的实现：注册 → 条件过滤 → 建图 → 拓扑排序
//! → 创建并立即初始化 → 容器就绪批次（after_all_ready），
//! 以及销毁前批次（before_destroy）与逆序销毁。
//!
//! 与容器查询 API（见 [`super`]）分离：装配是启动期一次性过程，
//! 查询是运行期高频 API。加载计划等启动期局部数据在加载完成后即释放。

use super::{
    ComponentContainer, ComponentKey, ComponentLifecycle, ComponentProcessor,
    ComponentProcessorFactory, PrimaryRegistration, TraitImplRegistration,
};
use super::lifecycle::{build_lifecycle_index, LifecycleRegistration};
use crate::model::condition::{ComponentCondition, ConditionContext};
use crate::model::context::global_context::{clear_context_snapshot, install_context_snapshot};
use crate::utils::inner_util::{
    build_component_indexes, build_impl_registration_index, build_trait_impl_index, find_cycle_path,
};
use anyhow::{Context, anyhow};
use std::any::TypeId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use toml::Value;

// =============================================================================
// ComponentContainer 生命周期方法（加载与销毁）
// =============================================================================

impl ComponentContainer {
    /// 加载并初始化所有组件
    ///
    /// 包含四个步骤：
    /// 1. 注册阶段：扫描 inventory 工厂、校验名称唯一、条件过滤，构建创建阶段索引。
    /// 2. 创建与初始化阶段：建图展开四类依赖（名称/trait/类型/primary）→ Kahn 拓扑
    ///    排序 → 按序创建，每个组件创建完成后**立即初始化**（init 语义：注入完成后
    ///    即执行，对应 Spring `@PostConstruct`；拓扑序保证依赖组件先完成初始化）。
    /// 3. 容器就绪阶段：按创建顺序批次执行 `ComponentLifecycle::after_all_ready`
    ///    （框架内置组件与用户组件的收尾均在此批次完成，如默认事件发布器的
    ///    监听器收集）。
    ///
    /// 采用 arc-self 接收者：create 回调需要 `Arc<ComponentContainer>` 做构造器注入，
    /// 此处直接向下传递自身的 Arc 克隆。
    /// `config` 为合并后的全局配置（由 `Application` 持有并传入，配置组件与
    /// 条件评估使用）。
    pub(crate) async fn load(self: &Arc<Self>, config: Arc<Value>) -> anyhow::Result<()> {
        let plan = register_components(self, &config)?;
        run_creation(plan, self, &config).await?;
        // 全部组件创建与初始化完成：冻结全部存储为不可变快照，
        // 此后运行期查询零锁读，装配期写被类型层面拒绝
        self.freeze_all();
        // 容器已冻结：安装全局上下文快照（`after_all_ready` 批次前），
        // 批次回调与运行期任意线程均可经 `app_container` / `app_config` 读取
        install_context_snapshot(self, config);
        run_after_all_ready(self).await?;
        Ok(())
    }

    /// 冻结全部存储为不可变快照（装配期 → 运行期的分界点）
    fn freeze_all(&self) {
        self.repository.freeze();
        self.trait_obj_cache.freeze();
        self.type_instance_names.freeze();
        self.trait_instance_names.freeze();
        self.primary_by_type.freeze();
        self.order.freeze();
    }

    /// 关闭并销毁所有组件
    ///
    /// 销毁顺序为创建顺序的逆序（后创建者先销毁，保证依赖方向安全）。
    /// 销毁循环前先批次执行容器级 `before_destroy`（此时全局缓存未清空、
    /// 全部 bean 存活可互相解析）。
    /// 采用 arc-self 接收者：before_destroy 回调需要 `&Arc<ComponentContainer>`
    /// （容器以 Arc 建模共享访问，回调签名与 `AppContext::container()` 形态一致）。
    pub(crate) async fn shutdown(self: &Arc<Self>) -> anyhow::Result<()> {
        // 1. 取回创建顺序列表（take 幂等：防止多次 shutdown 重复执行）
        let sorted_keys: Vec<ComponentKey> = self.order.take().unwrap_or_default();

        if sorted_keys.is_empty() {
            tracing::warn!("No components to shutdown or creation order is already empty.");
            return Ok(());
        }

        // 2. 容器级 before_destroy 批次（销毁循环前、全局缓存清空前执行：
        //    全部 bean 存活、缓存完整，回调内可解析任意依赖；失败记日志不中断）
        run_before_destroy(self, &sorted_keys).await;

        // 3. 清空全局上下文快照（before_destroy 批次后、销毁循环前）：
        //     此后 `app_container` / `app_config` 返回 None，销毁循环期间
        //     迟到读者安全失败，不会触达已掏空的容器
        clear_context_snapshot();

        // 4. 销毁前先清空 trait object 缓存与实例名索引，释放对组件实例的额外引用，
        //    确保后续 destroy() 中 Arc::try_unwrap 的 refcount 为 1。
        self.trait_obj_cache.clear();
        self.type_instance_names.clear();
        self.trait_instance_names.clear();
        self.primary_by_type.clear();

        // 5. 取回仓库所有权，逆序移除并调用 destroy
        let mut repo = self.repository.take().unwrap_or_default();
        for key in sorted_keys.iter().rev() {
            if let Some(mut processor) = repo.remove(key) {
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
    /// 具体类型 → 实例名列表 索引（trait 依赖展开使用，避免建图期间扫描仓库）
    type_instance_index: HashMap<TypeId, Vec<String>>,
    /// 实现类型 → 匹配的 trait 实现注册列表（一对多，populate 填充缓存用）
    impl_registration_index: HashMap<TypeId, Vec<&'static TraitImplRegistration>>,
}

/// 单个组件的创建计划（依赖声明来自编译期工厂元数据）
struct PlanEntry {
    /// 具体类型依赖（组件名列表）
    dependencies: Vec<&'static str>,
    /// trait 依赖的 TypeId 列表（建图时展开为全部实现组件）
    trait_dependencies: Vec<TypeId>,
    /// 具体类型依赖的 TypeId 列表（建图时展开为该类型全部实例；
    /// 无名称注入 `Arc<T>` 时使用，组件可能自定义名称）
    type_dependencies: Vec<TypeId>,
    /// primary 依赖的 TypeId 列表（`#[inject_primary]` 生成；
    /// 建图时仅建边到该类型的 primary 实例，不强制创建同类型其他实例）
    primary_dependencies: Vec<TypeId>,
    /// 条件声明：None 无条件注册；Some 为注册期已求值的条件
    condition: Option<ComponentCondition>,
}

/// 注册组件阶段
///
/// 遍历 inventory 收集的工厂，校验名称唯一性，存入容器仓库，
/// 构建创建阶段所需的索引并返回加载计划。
fn register_components(
    container: &Arc<ComponentContainer>,
    config: &Value,
) -> anyhow::Result<LoadPlan> {
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

        container
            .repository
            .get_mut()
            .expect("repository must be mutable during registration")
            .insert(name.to_string(), processor);

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

    // 5. 注册期条件过滤（不满足的组件从仓库与计划中移除，不参与创建）
    filter_components_by_condition(&mut entries, container, config)?;

    // 6. 构建 primary 索引（条件过滤后，校验 primary 名字对应组件存在、同类型唯一）
    build_primary_index(container)?;

    // 7. 构建创建阶段所需索引（启动期局部数据，加载完成后即释放）
    let trait_impl_index = build_trait_impl_index();
    let impl_registration_index = build_impl_registration_index();
    // 组件名快照已由 entries 的 key 集表达（条件过滤同步移除仓库与计划，两者等价）
    let (_, type_instance_index) = build_component_indexes(
        container
            .repository
            .get()
            .expect("repository must be initialized during registration"),
    );

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
/// 计划中移除，使其不参与后续创建。条件仅依赖"注册信息 + 全局配置"，
/// 评估结果与组件创建顺序无关（对齐 Spring 的 bean definition 期条件评估语义）。
///
/// 单轮评估 + 全量注册快照：评估开始时的快照在整个评估过程中保持不变，
/// 链式互斥条件（A 条件是 B 不存在、B 条件是 A 不存在）两者都注册，
/// 不做不动点迭代，语义可预测优先。
///
/// 仅 inventory 组件携带条件声明。
fn filter_components_by_condition(
    entries: &mut HashMap<String, PlanEntry>,
    container: &Arc<ComponentContainer>,
    config: &Value,
) -> anyhow::Result<()> {
    // 快速路径：无任何条件声明
    if !entries.values().any(|e| e.condition.is_some()) {
        return Ok(());
    }

    // 1. 构建注册全量快照（单轮评估的固定上下文）
    let ctx = ConditionContext::snapshot(container);

    // 2. 单轮评估，收集不满足者
    let mut to_remove: Vec<String> = Vec::new();
    for (name, entry) in entries.iter() {
        if let Some(condition) = &entry.condition
            && !condition.evaluate(&ctx, name, config)
        {
            to_remove.push(name.clone());
        }
    }

    // 3. 统一移除：仓库 + 创建计划（创建阶段索引在过滤后构建，天然不含被移除组件）
    for name in &to_remove {
        container
            .repository
            .get_mut()
            .expect("repository must be mutable during condition filtering")
            .remove(name);
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
fn build_primary_index(container: &ComponentContainer) -> anyhow::Result<()> {
    for reg in inventory::iter::<PrimaryRegistration> {
        let type_id = reg.type_id;

        // 校验：primary 指向的组件必须已注册（条件过滤后仍存在）
        if !container
            .repository
            .get()
            .is_some_and(|r| r.contains_key(reg.name))
        {
            return Err(anyhow!(
                "Primary instance '{}' is not registered. #[primary] name must match a registered provider component name.",
                reg.name
            ));
        }

        // 校验：同类型 primary 唯一
        if let Some(existing) = container
            .primary_by_type
            .get_mut()
            .expect("primary_by_type must be mutable during registration")
            .insert(type_id, reg.name.to_string())
        {
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
// 创建阶段（建图 → 拓扑排序 → 按序创建）
// =============================================================================

/// 创建阶段依赖图（启动期局部数据，加载完成即释放）
///
/// 边方向：依赖项 → 依赖它的组件。Kahn 排序时依赖项先出队，
/// 出队后将其所有依赖者的入度减一，减到 0 的依赖者入队，
/// 天然保证"依赖先于依赖者"的创建顺序。
struct CreationGraph {
    /// 依赖项组件名 → 依赖它的组件列表（Kahn 出队后传播减度）
    dependents: HashMap<String, Vec<String>>,
    /// 组件名 → 未满足的依赖数量（Kahn 入度，减到 0 即可创建）
    in_degree: HashMap<String, usize>,
}

/// 阶段一：建图 + 拓扑排序 + 按序创建组件（创建后立即初始化）
///
/// 环在创建任何组件之前一次性检出（排序结果数小于组件总数即有环），
/// 创建失败不会残留部分已创建的组件状态。
/// 每个组件创建完成后**立即 init**（init 语义：注入完成后即执行，对应 Spring
/// `@PostConstruct`；拓扑序保证依赖组件先完成初始化）。
async fn run_creation(
    plan: LoadPlan,
    container: &Arc<ComponentContainer>,
    config: &Value,
) -> anyhow::Result<()> {
    // 1. 建图：展开四类依赖为边（依赖项 → 依赖者）
    let graph = build_creation_graph(&plan, container)?;

    // 2. Kahn 拓扑排序：得到"依赖先于依赖者"的创建顺序
    let order = topo_sort_creation(&graph)?;

    // 3. 按序迭代创建，创建完成后立即初始化
    for name in &order {
        create_one(container, name, &plan, config).await?;
        init_one(container, name).await?;
    }

    Ok(())
}

/// 建图：把每个组件的四类依赖声明展开为具体组件名，构建依赖边
///
/// 依赖声明来自注册期快照，创建期无动态依赖：
/// - 名称依赖：直接建边（依赖未注册 fail-fast）
/// - trait 依赖：展开为全部实现组件的全部实例（无实现/无实例 fail-fast）
/// - 类型依赖：展开为该类型的全部实例（无实例 fail-fast）
/// - primary 依赖：仅建边到该类型的 primary 实例（无声明 fail-fast）
///
/// 四类依赖在建边时统一去重合并，否则入度与重复边都会失真。
fn build_creation_graph(
    plan: &LoadPlan,
    container: &ComponentContainer,
) -> anyhow::Result<CreationGraph> {
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();

    for name in plan.entries.keys() {
        let entry = &plan.entries[name];
        // 去重后的直接依赖集合（四类依赖可能指向同一组件，合并为唯一边）
        let mut deps: HashSet<String> = HashSet::new();

        // 1. 名称依赖：依赖未注册时在此 fail-fast
        for dep_name in &entry.dependencies {
            if !plan.entries.contains_key(*dep_name) {
                return Err(anyhow!(
                    "Component depends on '{}', but '{}' is not registered.",
                    dep_name,
                    dep_name
                ));
            }
            deps.insert(dep_name.to_string());
        }

        // 2. trait 依赖：展开为全部实现组件的全部实例
        for trait_type_id in &entry.trait_dependencies {
            let impls = plan
                .trait_impl_index
                .get(trait_type_id)
                .ok_or_else(|| {
                    anyhow!(
                        "Component '{}' depends on a trait (TypeId={:?}) that has no registered implementations",
                        name,
                        trait_type_id
                    )
                })?;

            let mut resolved_any = false;
            for impl_type_id in impls.iter() {
                let instance_names = plan
                    .type_instance_index
                    .get(impl_type_id)
                    .ok_or_else(|| {
                        anyhow!(
                            "Component '{}' depends on a trait (TypeId={:?}), but its implementation type {:?} is not registered",
                            name,
                            trait_type_id,
                            impl_type_id
                        )
                    })?;
                for impl_name in instance_names {
                    deps.insert(impl_name.clone());
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

        // 3. 类型依赖：展开为该类型的所有实例
        for type_id in &entry.type_dependencies {
            let instance_names = plan
                .type_instance_index
                .get(type_id)
                .ok_or_else(|| {
                    anyhow!(
                        "Component '{}' depends on type (TypeId={:?}) that has no registered component instances",
                        name,
                        type_id
                    )
                })?;
            for impl_name in instance_names {
                deps.insert(impl_name.clone());
            }
        }

        // 4. primary 依赖：仅建边到该类型的 primary 实例
        for type_id in &entry.primary_dependencies {
            // 注册期 build_primary_index 已校验 primary 名对应的组件存在
            let primary_name = container
                .primary_by_type
                .get()
                .and_then(|m| m.get(type_id))
                .ok_or_else(|| {
                    anyhow!(
                        "Component '{}' depends on a primary instance (TypeId={:?}) that is not registered; a #[primary] must be declared on one of the type's providers",
                        name,
                        type_id
                    )
                })?;
            deps.insert(primary_name.clone());
        }

        // 入度 = 去重后的直接依赖数；反向登记依赖者
        in_degree.insert(name.clone(), deps.len());
        for dep in deps {
            dependents.entry(dep).or_default().push(name.clone());
        }
    }

    Ok(CreationGraph {
        dependents,
        in_degree,
    })
}

/// Kahn 拓扑排序：入度为 0 者先创建，出队后传播减度
///
/// 返回创建顺序（依赖先于依赖者）。存在环时排序结果数小于组件总数，
/// 调用 `find_cycle_path` 生成可读的环路径后 fail-fast。
fn topo_sort_creation(graph: &CreationGraph) -> anyhow::Result<Vec<String>> {
    let total = graph.in_degree.len();
    let mut in_degree = graph.in_degree.clone();
    let mut order: Vec<String> = Vec::with_capacity(total);

    // 双端队列（头出尾进）：初始入队所有无依赖组件
    let mut queue: VecDeque<String> = VecDeque::new();
    for (name, degree) in in_degree.iter() {
        if *degree == 0 {
            queue.push_back(name.clone());
        }
    }

    while let Some(name) = queue.pop_front() {
        order.push(name.clone());
        // 该组件即将创建，其依赖者的依赖数减一，减到 0 即可创建
        if let Some(dependents) = graph.dependents.get(&name) {
            for dependent in dependents {
                let degree = in_degree
                    .get_mut(dependent)
                    .ok_or_else(|| anyhow!("Component '{}' has no in-degree entry", dependent))?;
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(dependent.clone());
                }
            }
        }
    }

    // 有环：剩余入度 > 0 的节点即环成员或依赖环的节点，生成可读路径后报错
    if order.len() < total {
        return Err(anyhow!(
            "Circular dependency detected in components: [{}]",
            find_cycle_path(&graph.dependents, &in_degree)
        ));
    }

    Ok(order)
}

/// 创建单个组件
///
/// 采用 temporarily remove 模式执行 create：先取出处理器所有权（释放对仓库
/// 冻结单元的借用），await 期间 create 回调可读容器做构造器注入（FreezeCell
/// BUILDING 期读写不得重叠的契约），完成后插回。
/// 创建完成后立即填充 trait object 缓存（依赖者 create 阶段即可
/// 按 trait 获取），并记录创建顺序（销毁时逆序使用）。
async fn create_one(
    container: &Arc<ComponentContainer>,
    name: &str,
    plan: &LoadPlan,
    config: &Value,
) -> anyhow::Result<()> {
    let mut processor = container
        .repository
        .get_mut()
        .expect("repository must be mutable during creation")
        .remove(name)
        .ok_or_else(|| anyhow!("Component '{}' not found in repository", name))?;

    // create 回调经 Arc<ComponentContainer> 访问容器做构造器注入，
    // 配置以 Arc 克隆传入（配置组件反序列化使用）
    let create_result = processor
        .create(Arc::clone(container), Arc::new(config.clone()))
        .await
        .with_context(|| format!("Failed to create component: {}", name));
    container
        .repository
        .get_mut()
        .expect("repository must be mutable during creation")
        .insert(name.to_string(), processor);
    create_result?;

    tracing::debug!("Component created: {}", name);

    // 创建后立即缓存该组件的 trait object，
    // 确保后续组件在 create 阶段即可通过 get_component_by_trait 获取依赖
    populate_trait_obj_cache(container, &name.to_string(), &plan.impl_registration_index)?;

    // 记录创建顺序（销毁时逆序使用）
    container
        .order
        .get_mut()
        .expect("order must be mutable during creation")
        .push(name.to_string());

    Ok(())
}

// =============================================================================
// 初始化与容器级生命周期批次
// =============================================================================

/// 初始化单个组件
///
/// 在 [`create_one`] 之后立即调用（init 语义：注入完成后即执行，对应 Spring
/// `@PostConstruct`）。采用 temporarily remove 模式执行 init：先取出处理器
/// 所有权（释放对仓库冻结单元的借用），await 期间 init 回调可读容器
/// （FreezeCell BUILDING 期读写不得重叠的契约），完成后插回。
async fn init_one(container: &ComponentContainer, name: &str) -> anyhow::Result<()> {
    let mut processor = container
        .repository
        .get_mut()
        .expect("repository must be mutable during init")
        .remove(name)
        .ok_or_else(|| anyhow!("Component '{}' not found in repository", name))?;

    let init_result = processor
        .init()
        .await
        .with_context(|| format!("Failed to init component: {}", name));
    container
        .repository
        .get_mut()
        .expect("repository must be mutable during init")
        .insert(name.to_string(), processor);
    init_result?;

    tracing::debug!("Component initialized: {}", name);
    Ok(())
}

/// 按组件 key 还原该组件类型注册的容器级生命周期实现
///
/// 三步：仓库取处理器 → 按处理器具体类型查生命周期索引 → accessor 将
/// 类型擦除实例还原为 `Arc<dyn ComponentLifecycle>`。组件不存在或该类型
/// 未注册生命周期实现时返回 `None`（均按“无回调”处理）。
/// 同步函数：repository 读守卫在函数内作用域收窄、不跨 await
/// （回调内可能再次查询容器），无需 temporarily remove。
fn resolve_lifecycle(
    container: &ComponentContainer,
    lifecycle_index: &HashMap<TypeId, Vec<&'static LifecycleRegistration>>,
    key: &ComponentKey,
) -> Option<Arc<dyn ComponentLifecycle>> {
    let processor = container.repository.get()?.get(key)?;
    let regs = lifecycle_index.get(&ComponentProcessor::type_id(&**processor))?;
    let arc_any = processor.get_inner_arc_any()?;
    regs.iter().find_map(|reg| (reg.accessor)(arc_any.clone()))
}

/// 阶段三：容器级 after_all_ready 批次（全部组件创建与初始化完成后）
///
/// 按创建顺序正序遍历，命中 [`LifecycleRegistration`]
/// 的组件执行 `ComponentLifecycle::after_all_ready`（对应 Spring
/// `SmartInitializingSingleton`）。
/// 回调为共享引用（`&self`），repository 读守卫在闭包内作用域收窄、
/// 不跨 await（回调内可能再次查询容器），无需 temporarily remove。
/// 启动期回调失败 fail-fast。
async fn run_after_all_ready(container: &Arc<ComponentContainer>) -> anyhow::Result<()> {
    let lifecycle_index = build_lifecycle_index();
    if lifecycle_index.is_empty() {
        return Ok(());
    }

    let sorted_keys: Vec<ComponentKey> = container.order.get().cloned().unwrap_or_default();

    for key in &sorted_keys {
        if let Some(lifecycle) = resolve_lifecycle(container, &lifecycle_index, key) {
            lifecycle
                .after_all_ready(container)
                .await
                .with_context(|| format!("Failed to run after_all_ready for component: {}", key))?;
        }
    }

    Ok(())
}

/// 容器级 before_destroy 批次（销毁循环前、全局缓存清空前执行）
///
/// 按创建顺序逆序批次执行（与 destroy 顺序一致）：此时全部 bean 存活、
/// 全局缓存完整，回调内可解析任意依赖（含依赖者——destroy 逆序执行时
/// 组件自身的 destroy 回调执行前依赖者已被销毁，本批次无此限制）。
/// 关闭期回调失败仅记日志，不中断销毁流程。
async fn run_before_destroy(container: &Arc<ComponentContainer>, sorted_keys: &[ComponentKey]) {
    let lifecycle_index = build_lifecycle_index();
    if lifecycle_index.is_empty() {
        return;
    }

    for key in sorted_keys.iter().rev() {
        if let Some(lifecycle) = resolve_lifecycle(container, &lifecycle_index, key) {
            if let Err(e) = lifecycle.before_destroy(container).await {
                tracing::error!("Error running before_destroy for component '{}': {:?}", key, e);
            }
        }
    }
}

// =============================================================================
// trait object 缓存填充与事件发布器收集
// =============================================================================

/// 为指定组件填充 trait object 缓存与实例名索引
///
/// 通过实现类型注册索引（`impl_registration_index`，启动期构建的局部快照）
/// 按组件具体类型直接查询匹配的注册项（一对多：一个组件类型可注册多个 trait 实现），
/// 通过 accessor 将 `Arc<ConcreteType>` 转换为 `Arc<dyn Injectable>`，
/// 以 **(trait_type_id, 组件实例名)** 为 key 存入容器的 `trait_obj_cache`。
///
/// 同步填充两个运行时实例名索引：
/// - `type_instance_names`：类型维度（每个组件 create 后必填）
/// - `trait_instance_names`：trait 维度（accessor 命中时填充）
///
/// 使用组件实例名作为 cache key（而非 TraitImplRegistration 名称），
/// 确保同一具体类型的多个实例（如通过 provider 创建的同类型不同名称组件）
/// 各自拥有独立的 cache 条目。
fn populate_trait_obj_cache(
    container: &ComponentContainer,
    key: &ComponentKey,
    impl_registration_index: &HashMap<TypeId, Vec<&'static TraitImplRegistration>>,
) -> anyhow::Result<()> {
    let processor = container
        .repository
        .get()
        .and_then(|r| r.get(key))
        .ok_or_else(|| anyhow::anyhow!("Component '{}' not found in repository", key))?;

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
        let names_entry = container
            .type_instance_names
            .get_mut()
            .expect("type_instance_names must be mutable during creation")
            .entry(component_type_id)
            .or_default();
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
                container
                    .trait_obj_cache
                    .get_mut()
                    .expect("trait_obj_cache must be mutable during creation")
                    .insert(cache_key, entry);
                {
                    let names_entry = container
                        .trait_instance_names
                        .get_mut()
                        .expect("trait_instance_names must be mutable during creation")
                        .entry(reg.trait_type_id)
                        .or_default();
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
