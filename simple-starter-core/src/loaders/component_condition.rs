//! # 组件条件注册模型
//!
//! 提供组件"条件注册"支持：条件在**注册期**统一评估（对齐 Spring bean
//! definition 期 `@Conditional` 语义），评估结果与组件创建顺序无关。
//!
//! 流程：inventory 工厂全量登记（含惰性条件构造函数）→ `filter_components_by_condition`
//! 求值条件并构建注册全量快照单轮评估 → 不满足者从仓库统一移除 → 构建创建阶段索引。

use crate::utils::app_core_util::AppCoreUtil;
use crate::utils::app_inner_util::{build_component_indexes, build_trait_impl_index};
use std::any::TypeId;
use std::collections::{HashMap, HashSet};

/// 组件条件声明
///
/// 由 `#[component(condition = ...)]` 宏参数生成，经工厂的惰性构造函数
/// 在注册期求值一次，随后在组件创建前统一评估。
#[derive(Clone, Copy)]
pub enum ComponentCondition {
    /// 无其他已注册组件是此具体类型（"默认实现 + 用户覆盖"场景）
    OnMissingType(fn() -> TypeId),
    /// 无其他已注册组件实现此 trait（trait 替换默认实现场景）
    OnMissingTrait(fn() -> TypeId),
    /// 全局配置中存在指定点分路径的键（可选期望值）
    OnProperty {
        key: &'static str,
        expected: Option<&'static str>,
    },
    /// 用户自定义条件函数
    Custom(fn(&ConditionContext) -> bool),
}

impl ComponentCondition {
    /// 条件：无其他已注册组件是类型 `T`
    pub fn on_missing_type<T: 'static>() -> Self {
        Self::OnMissingType(TypeId::of::<T>)
    }

    /// 条件：无其他已注册组件实现 trait `T`（传入 `dyn Trait`）
    pub fn on_missing_trait<T: ?Sized + 'static>() -> Self {
        Self::OnMissingTrait(TypeId::of::<T>)
    }

    /// 条件：全局配置中存在该点分路径键
    pub fn on_property(key: &'static str) -> Self {
        Self::OnProperty { key, expected: None }
    }

    /// 条件：全局配置中该键的字符串值等于期望值
    pub fn on_property_eq(key: &'static str, expected: &'static str) -> Self {
        Self::OnProperty {
            key,
            expected: Some(expected),
        }
    }

    /// 评估条件
    ///
    /// `candidate` 为当前被评估的组件名，用于排除"组件自己"——
    /// 例如默认实现的条件是"无其他实现"，不应把自己计入已注册集合。
    pub(crate) fn evaluate(&self, ctx: &ConditionContext, candidate: &str) -> bool {
        match self {
            Self::OnMissingType(get_type_id) => {
                let target = get_type_id();
                !ctx.has_other_instance_of_type(&target, candidate)
            }
            Self::OnMissingTrait(get_trait_id) => {
                let target = get_trait_id();
                let Some(impl_types) = ctx.registered_trait_impls.get(&target) else {
                    return true;
                };
                !impl_types
                    .iter()
                    .any(|impl_type| ctx.has_other_instance_of_type(impl_type, candidate))
            }
            Self::OnProperty { key, expected } => {
                let Some(value) = AppCoreUtil::get_config_value_by_path(key) else {
                    return false;
                };
                match expected {
                    Some(exp) => value.as_str().is_some_and(|s| s == *exp),
                    None => true,
                }
            }
            Self::Custom(evaluate_fn) => evaluate_fn(ctx),
        }
    }
}

/// 条件评估上下文（注册全量快照）
///
/// 在评估开始前一次性构建，评估过程中保持不变：保证单轮评估的结果
/// 与评估顺序无关（链式互斥条件不做不动点迭代，语义可预测优先）。
pub struct ConditionContext {
    /// 全量已注册组件名快照
    registered_names: HashSet<String>,
    /// 已注册具体类型 → 实例名列表
    type_instance_index: HashMap<TypeId, Vec<String>>,
    /// trait → 已注册实现类型列表（inventory 全量注册，与已注册组件求交使用）
    registered_trait_impls: HashMap<TypeId, Vec<TypeId>>,
}

impl ConditionContext {
    /// 构建注册全量快照（仓库 + inventory trait 注册）
    ///
    /// 仅在组件加载的注册期调用（组件创建前，单线程启动阶段），无锁竞争风险。
    pub(crate) fn snapshot() -> Self {
        // 仓库全量快照（与创建阶段索引共用同一构建工具；此处为过滤前全量语义）
        let (registered_names, type_instance_index) = build_component_indexes();
        // trait → 已注册实现类型列表（与建图 trait 展开共用同一索引构建工具）
        let registered_trait_impls = build_trait_impl_index();

        Self {
            registered_names,
            type_instance_index,
            registered_trait_impls,
        }
    }

    /// 指定名字的组件是否已注册（自定义条件查询用）
    pub fn has_component(&self, name: &str) -> bool {
        self.registered_names.contains(name)
    }

    /// 指定具体类型是否已注册（自定义条件查询用）
    pub fn has_type(&self, type_id: &TypeId) -> bool {
        self.type_instance_index.contains_key(type_id)
    }

    /// 该具体类型是否存在除 `candidate` 之外的其他已注册实例
    fn has_other_instance_of_type(&self, type_id: &TypeId, candidate: &str) -> bool {
        self.type_instance_index
            .get(type_id)
            .is_some_and(|names| names.iter().any(|n| n != candidate))
    }
}
