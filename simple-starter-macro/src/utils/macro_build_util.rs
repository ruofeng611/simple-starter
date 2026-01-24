use quote::ToTokens;
use syn::parse::ParseStream;
use syn::{Attribute, GenericArgument, Ident, LitStr, Meta, PathArguments, Token, Type};

/// 从 Type AST 中提取“短类型名”（即路径的最后一段）。
///
/// # 用途
/// 用于依赖注入系统中，根据类型名称生成唯一的依赖标识符。
/// 例如：`std::sync::Arc<MyService>` -> `"MyService"`。
pub(crate) fn get_short_type_name_from_type(ty: &Type) -> String {
    if let Type::Path(type_path) = ty {
        // 获取路径段的最后一个标识符
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident.to_string();
        }
    }
    // 回退策略：对于无法解析为 Path 的复杂类型，直接转换为字符串表示
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
