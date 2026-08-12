use quote::ToTokens;
use syn::parse::ParseStream;
use syn::{
    Attribute, GenericArgument, Ident, LitStr, Meta, PathArguments, Token, Type,
    TypeTraitObject,
};

/// 从 Type AST 中提取"短类型名"（即路径的最后一段）。
///
/// # 用途
/// 用于依赖注入系统中，根据类型名称生成唯一的依赖标识符。
/// 例如：`std::sync::Arc<MyService>` -> `"MyService"`。
pub(crate) fn get_short_type_name_from_type(ty: &Type) -> String {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident.to_string();
        }
    }
    if let Type::TraitObject(trait_obj) = ty {
        if let Some(syn::TypeParamBound::Trait(trait_bound)) = trait_obj.bounds.first() {
            if let Some(segment) = trait_bound.path.segments.last() {
                return segment.ident.to_string();
            }
        }
    }
    // 回退策略：对于无法解析的复杂类型，直接转换为字符串表示
    ty.to_token_stream().to_string()
}

/// 检查类型是否为 `Arc<T>` 并提取其中的 `T`。
///
/// # 用途
/// 依赖注入系统要求所有共享组件必须包裹在 `Arc` 中。
/// 此函数用于验证字段类型并获取内部真实类型。
///
/// # 返回值
/// - `Some(&Type)`: 如果是 `Arc<T>`，返回 `T`。
/// - `None`: 如果不是 `Arc` 类型。
pub(crate) fn get_arc_inner_type(ty: &Type) -> Option<&Type> {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            // 检查类型名是否为 "Arc"
            if segment.ident == "Arc" {
                // 解析尖括号参数 <T>
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner_ty)) = args.args.first() {
                        return Some(inner_ty);
                    }
                }
            }
        }
    }
    None
}

/// 解析并移除字段或参数上的 `#[inject]` 属性。
///
/// # 支持格式
/// - `#[inject]`: 按类型注入
/// - `#[inject("name")]`: 按名称注入（简写）
/// - `#[inject(name = "name")]`: 按名称注入（键值对）
///
/// # 返回值
/// - `bool`: 是否存在 `#[inject]` 属性。
/// - `Option<String>`: 指定的注入名称（如果有）。
pub(crate) fn parse_and_strip_inject(attrs: &mut Vec<Attribute>) -> (bool, Option<String>) {
    let mut is_injected = false;
    let mut inject_name = None;
    let mut indices_to_remove = Vec::new();

    // 1. 遍历属性寻找 #[inject]
    for (i, attr) in attrs.iter().enumerate() {
        if attr.path().is_ident("inject") {
            is_injected = true;
            indices_to_remove.push(i);

            // 解析属性内容
            if let Meta::List(meta) = &attr.meta {
                let _ = meta.parse_args_with(|input: ParseStream| {
                    if input.is_empty() {
                        return Ok(());
                    }

                    // 情况 A: 直接字符串 #[inject("foo")]
                    if let Ok(lit) = input.parse::<LitStr>() {
                        inject_name = Some(lit.value());
                        return Ok(());
                    }

                    // 情况 B: 键值对 #[inject(name="foo")]
                    if input.peek(Ident) {
                        let key: Ident = input.parse()?;
                        if key == "name" {
                            let _: Token![=] = input.parse()?;
                            let value: LitStr = input.parse()?;
                            inject_name = Some(value.value());
                            return Ok(());
                        }
                    }

                    Ok(())
                });
            }
        }
    }

    // 2. 移除 #[inject] 属性
    // 必须倒序移除，以防止索引偏移导致移除错误的属性
    for i in indices_to_remove.into_iter().rev() {
        attrs.remove(i);
    }

    (is_injected, inject_name)
}

/// 组合基础路径和方法路径。
///
/// 用于 `rest_controller` 和 `security_controller` 宏中拼接完整路由路径。
pub(crate) fn combine_paths(base: &str, method: &str) -> String {
    let base = base.trim_matches('/');
    let method = method.trim_matches('/');

    if base.is_empty() && method.is_empty() {
        "/".to_string()
    } else if base.is_empty() {
        format!("/{}", method)
    } else if method.is_empty() {
        format!("/{}", base)
    } else {
        format!("/{}/{}", base, method)
    }
}

// =============================================================================
// Trait Object 注入相关的类型检测工具
// =============================================================================

/// 从类型中提取 dyn trait 对象（例如从 `Arc<dyn MyTrait>` 中提取 `dyn MyTrait`）。
/// 该函数尝试从外层的 `Arc<...>` 中剥离出内部的 `dyn Trait`。
///
/// 返回: 内部 trait object 的类型引用
pub(crate) fn get_dyn_trait_in_arc(ty: &Type) -> Option<&TypeTraitObject> {
    // 解包 Arc<...>
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Arc" {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner_ty)) = args.args.first() {
                        if let Type::TraitObject(trait_obj) = inner_ty {
                            return Some(trait_obj);
                        }
                    }
                }
            }
        }
    }
    None
}

/// 检测类型是否为 `Arc<dyn Trait>` 形式
pub(crate) fn is_arc_dyn_trait(ty: &Type) -> bool {
    get_dyn_trait_in_arc(ty).is_some()
}

/// 检测类型是否为 `Vec<Arc<dyn Trait>>` 形式，并提取内部的 `dyn Trait`
///
/// 返回: 内部 trait object 的类型引用
pub(crate) fn get_dyn_trait_in_vec_arc(ty: &Type) -> Option<&TypeTraitObject> {
    // 解包 Vec<...>
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Vec" {
                if let PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(GenericArgument::Type(inner_ty)) = args.args.first() {
                        // 再解包 Arc<dyn Trait>
                        return get_dyn_trait_in_arc(inner_ty);
                    }
                }
            }
        }
    }
    None
}

/// 检测类型是否为 `Vec<Arc<dyn Trait>>` 形式
pub(crate) fn is_vec_arc_dyn_trait(ty: &Type) -> bool {
    get_dyn_trait_in_vec_arc(ty).is_some()
}

/// 将 `TypeTraitObject` 转回 `Type`（用于代码生成中的 turbofish）
pub(crate) fn trait_object_to_type(trait_obj: &TypeTraitObject) -> Type {
    Type::TraitObject(trait_obj.clone())
}
