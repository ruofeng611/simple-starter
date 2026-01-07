use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Expr, ItemFn, Lit, Meta, ReturnType, Type, TypePath,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

/// 解析 `#[auto_component]` 或 `#[auto_component(name = "...")]` 的参数
struct AutoComponentArgs {
    /// 可选的自定义组件名；若未提供，则使用返回类型的名称
    name: Option<String>,
}

impl Parse for AutoComponentArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            // 无参数：#[auto_component]
            return Ok(AutoComponentArgs { name: None });
        }

        // 有参数：必须是 `name = "..."` 形式
        let meta = input.parse::<Meta>()?;
        match meta {
            Meta::NameValue(mnv) => {
                if mnv.path.is_ident("name") {
                    match &mnv.value {
                        Expr::Lit(expr_lit) => match &expr_lit.lit {
                            Lit::Str(s) => Ok(AutoComponentArgs {
                                name: Some(s.value()),
                            }),
                            _ => Err(syn::Error::new_spanned(
                                &expr_lit.lit,
                                "expected a string literal for `name`",
                            )),
                        },
                        _ => Err(syn::Error::new_spanned(
                            &mnv.value,
                            "expected a string literal",
                        )),
                    }
                } else {
                    Err(syn::Error::new_spanned(
                        mnv.path,
                        "unsupported attribute, expected `name = \"...\"`",
                    ))
                }
            }
            Meta::Path(_) | Meta::List(_) => {
                Err(syn::Error::new_spanned(meta, "expected `name = \"...\"`"))
            }
        }
    }
}

/// 检查函数签名是否包含 `self` 参数（即是否为方法）
fn has_self_param(sig: &syn::Signature) -> bool {
    sig.inputs
        .iter()
        .any(|arg| matches!(arg, syn::FnArg::Receiver(_)))
}

/// 从 Type 中递归提取最内层的 TypePath（支持引用、括号等）
fn extract_type_path(ty: &Type) -> &syn::Path {
    match ty {
        Type::Path(TypePath { path, .. }) => path,
        Type::Reference(r) => extract_type_path(&r.elem),
        Type::Paren(p) => extract_type_path(&p.elem),
        Type::Group(g) => extract_type_path(&g.elem),
        _ => panic!("Unsupported return type: expected a simple named type like `MyStruct`"),
    }
}

/// 实现 `#[auto_component]` 宏：将函数注册为组件工厂
///
/// 要求：
/// - 必须是无参函数（不能有 self）
/// - 必须有显式返回类型（如 `-> MyService`）
pub(crate) fn auto_component_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    // Step 1: 解析宏参数（是否有 name = "..."）
    let attr_args = parse_macro_input!(args as AutoComponentArgs);

    // Step 2: 解析被修饰的函数
    let input_fn = parse_macro_input!(input as ItemFn);
    let original_fn = &input_fn;
    let fn_name = &original_fn.sig.ident;

    // Step 3: 禁止用于方法（含 self）
    if has_self_param(&original_fn.sig) {
        return syn::Error::new_spanned(
            &original_fn.sig.fn_token,
            "`#[auto_component]` cannot be used on methods with `self`",
        )
        .to_compile_error()
        .into();
    }

    // Step 4: 检查返回类型是否存在
    let return_type = match &original_fn.sig.output {
        ReturnType::Type(_, ty) => ty.as_ref(),
        ReturnType::Default => {
            return syn::Error::new_spanned(
                &original_fn.sig.fn_token,
                "function must have an explicit return type",
            )
            .to_compile_error()
            .into();
        }
    };

    // Step 5: 提取返回类型的路径（如 MyService）
    let return_path = extract_type_path(return_type);

    // Step 6: 确定组件注册名：优先使用 name = "..."，否则用类型名
    let component_name = if let Some(ref custom) = attr_args.name {
        custom.clone()
    } else {
        return_path
            .segments
            .last()
            .expect("type path must have at least one segment")
            .ident
            .to_string()
    };

    // Step 7: 生成扩展代码：
    //   - 保留原始函数
    //   - 使用 无捕获闭包 直接构造组件实例并包装为 Arc<RwLock<T>>
    //   - 通过 `submit!` 宏注册到全局组件工厂表
    let expanded = quote! {
        // 保留用户定义的原始函数（必须存在，供闭包调用）
        #original_fn

        // 注册组件工厂：使用闭包作为构造器
        ::simple_starter_core::submit! {
            ::simple_starter_core::ComponentFactory {
                name: #component_name,
                // 存储组件的具体类型 ID，用于运行时类型匹配
                type_id: ::std::any::TypeId::of::<#return_path>(),
                // 构造器：无捕获闭包，等价于函数指针
                // 调用原函数，将其返回值包装为 Arc<RwLock<T>>，
                // 并转换为 dyn Any 以便统一存储和管理
                constructor: || {
                    ::std::sync::Arc::new(
                        ::std::sync::RwLock::new(#fn_name())
                    )
                },
            }
        }
    };

    TokenStream::from(expanded)
}
