use crate::utils::macro_build_util::{
    get_arc_inner_type, get_dyn_trait_in_arc, get_dyn_trait_in_vec_arc,
    get_result_inner_type, get_short_type_name_from_type, is_arc_dyn_trait,
    is_vec_arc_dyn_trait, parse_and_strip_inject, parse_and_strip_inject_primary,
    trait_object_to_type,
};
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::Parser;
use syn::{parse_macro_input, spanned::Spanned, FnArg, Ident, ItemFn, LitStr, ReturnType};

/// 实现 `#[provider]` 宏的核心逻辑。
///
/// # 功能
/// 将一个函数转换为组件工厂。适用于无法给结构体添加 `#[component]` 的场景（如第三方库类型）。
/// 函数参数会被自动处理为依赖注入。
///
/// # 参数
/// - `name`: 组件名称（可选）。
/// - `destroy_method`: 销毁逻辑，支持函数路径或闭包表达式。
pub(crate) fn provider_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut func = parse_macro_input!(input as ItemFn);

    // 1. 解析宏参数
    let (component_name, destroy_method, condition) = match parse_provider_args(args) {
        Ok(val) => val,
        Err(err) => return err.to_compile_error().into(),
    };

    // 2. 解析返回值类型 T
    // Provider 函数声明的完整返回类型 (例如: anyhow::Result<MyComponent>)
    let fn_return_type = match &func.sig.output {
        ReturnType::Type(_, ty) => ty.as_ref().clone(),
        ReturnType::Default => {
            return syn::Error::new(
                func.sig.span(),
                "Component provider function must return a type (e.g., anyhow::Result<T>)",
            )
                .to_compile_error()
                .into();
        }
    };

    // 尝试剥离 Result<T> 获取内部的 T。
    let component_type = get_result_inner_type(&fn_return_type)
        .cloned()
        .unwrap_or(fn_return_type.clone());

    // 确定组件名称 (使用剥离后的类型 T)
    let final_component_name = match component_name {
        Some(n) => n,
        None => get_short_type_name_from_type(&component_type),
    };

    // 3. 解析函数参数（处理依赖注入）
    let mut dependencies_names = Vec::new();
    let mut trait_dependency_type_ids: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut type_dependency_type_ids: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut primary_dependency_type_ids: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut arg_preparations = Vec::new(); // 生成从容器获取参数的代码
    let mut call_args = Vec::new(); // 生成函数调用时的参数列表

    for (i, arg) in func.sig.inputs.iter_mut().enumerate() {
        match arg {
            FnArg::Receiver(_) => {
                return syn::Error::new(
                    arg.span(),
                    "Component provider function cannot have 'self' argument",
                )
                    .to_compile_error()
                    .into();
            }
            FnArg::Typed(pat_type) => {
                // 处理参数上的 #[inject] / #[inject_primary]
                let (is_injected, inject_name) = parse_and_strip_inject(&mut pat_type.attrs);
                let is_primary = parse_and_strip_inject_primary(&mut pat_type.attrs);

                let arg_var_name = Ident::new(&format!("arg_{}", i), Span::call_site());

                // #[inject_primary] 单独使用即隐含注入语义；与 #[inject] 互斥
                if is_primary && is_injected {
                    return syn::Error::new(
                        pat_type.span(),
                        "#[inject_primary] cannot be combined with #[inject] on provider argument",
                    )
                    .to_compile_error()
                    .into();
                }

                // 根据参数类型选择注入策略
                if is_primary {
                    // primary 注入：仅允许具体类型 Arc<T>（primary 按具体类型维度注册）
                    if is_vec_arc_dyn_trait(&pat_type.ty) || is_arc_dyn_trait(&pat_type.ty) {
                        return syn::Error::new(
                            pat_type.ty.span(),
                            "#[inject_primary] on provider argument requires a concrete type `Arc<T>`; trait types are not supported (primary is registered on the concrete type dimension)",
                        )
                        .to_compile_error()
                        .into();
                    }

                    let inner_type = match get_arc_inner_type(&pat_type.ty) {
                        Some(ty) => ty,
                        None => {
                            return syn::Error::new(
                                pat_type.ty.span(),
                                "Provider argument marked with #[inject_primary] must be of type Arc<T>",
                            )
                            .to_compile_error()
                            .into();
                        }
                    };

                    primary_dependency_type_ids.push(quote! { ::std::any::TypeId::of::<#inner_type>() });
                    arg_preparations.push(quote! {
                        let #arg_var_name = ::simple_starter_core::AppCoreUtil::get_primary_component::<#inner_type>()?;
                    });
                } else if is_vec_arc_dyn_trait(&pat_type.ty) {
                    // Vec<Arc<dyn Trait>>
                    let trait_obj = get_dyn_trait_in_vec_arc(&pat_type.ty).unwrap();
                    let trait_type = trait_object_to_type(trait_obj);
                    trait_dependency_type_ids.push(quote! { ::std::any::TypeId::of::<#trait_type>() });
                    arg_preparations.push(quote! {
                        let #arg_var_name = ::simple_starter_core::AppCoreUtil::get_components_by_trait::<#trait_type>()?;
                    });

                } else if is_arc_dyn_trait(&pat_type.ty) {
                    // Arc<dyn Trait>
                    let trait_obj = get_dyn_trait_in_arc(&pat_type.ty).unwrap();
                    let trait_type = trait_object_to_type(trait_obj);

                    if let Some(name) = inject_name {
                        // 按名称注入：依赖明确，直接加名称即可，
                        // 不需要 trait TypeId 依赖（避免对其他无关实现建立虚假依赖边）
                        dependencies_names.push(name.clone());
                        arg_preparations.push(quote! {
                            let #arg_var_name = ::simple_starter_core::AppCoreUtil::get_component_by_trait_and_name::<#trait_type>(#name)?;
                        });
                    } else {
                        // 按 trait 注入：依赖所有实现，通过 TypeId 解析
                        trait_dependency_type_ids.push(quote! { ::std::any::TypeId::of::<#trait_type>() });
                        arg_preparations.push(quote! {
                            let #arg_var_name = ::simple_starter_core::AppCoreUtil::get_component_by_trait::<#trait_type>()?;
                        });
                    }

                } else {
                    // 普通 Arc<T>
                    let inner_type = get_arc_inner_type(&pat_type.ty).ok_or_else(|| {
                        syn::Error::new(
                            pat_type.ty.span(),
                            "Component provider arguments must be of type Arc<T>, Arc<dyn Trait>, or Vec<Arc<dyn Trait>>",
                        )
                    });

                    let inner_type = match inner_type {
                        Ok(t) => t,
                        Err(e) => return e.to_compile_error().into(),
                    };

                    if let Some(name) = inject_name {
                        dependencies_names.push(name.clone());
                        arg_preparations.push(quote! {
                            let #arg_var_name = ::simple_starter_core::AppCoreUtil::get_component_by_name::<#inner_type, _>(#name)?;
                        });
                    } else {
                        type_dependency_type_ids.push(quote! { ::std::any::TypeId::of::<#inner_type>() });
                        arg_preparations.push(quote! {
                            let #arg_var_name = ::simple_starter_core::AppCoreUtil::get_component::<#inner_type>()?;
                        });
                    }
                }

                call_args.push(arg_var_name);
            }
        }
    }

    // 4. 生成 Wrapper 构造逻辑
    let func_name = &func.sig.ident;

    // Create Fn: 包装原函数调用
    let create_fn_impl = quote! {
        Box::new(move || -> ::simple_starter_core::BoxFuture<::simple_starter_core::anyhow::Result<#component_type>> {
            Box::pin(async move {
                // 先准备所有参数
                #(#arg_preparations)*
                // 调用原 Provider 函数
                let instance = #func_name(#(#call_args),*).await?;
                Ok(instance)
            })
        })
    };

    // Destroy Fn: 处理销毁逻辑
    let destroy_fn_impl = if let Some(expr) = destroy_method {
        quote! {
            Some(Box::new(|c: #component_type| -> ::simple_starter_core::BoxFuture<::simple_starter_core::anyhow::Result<()>> {
                Box::pin(async move {
                    // 调用用户提供的销毁表达式 (可以是函数名或闭包)
                    let _ = (#expr)(c).await?;
                    Ok(())
                })
            }))
        }
    } else {
        quote! { None }
    };

    // 5. 生成条件声明（条件表达式包进惰性闭包，注册期求值一次）
    let condition_impl = match condition {
        Some(expr) => quote! { Some(|| #expr) },
        None => quote! { None },
    };

    // 6. 生成 Inventory 注册代码
    let inventory_impl = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_core::ComponentProcessorFactory {
                dependencies: &[#(#dependencies_names),*],
                trait_dependencies: &[#(#trait_dependency_type_ids),*],
                type_dependencies: &[#(#type_dependency_type_ids),*],
                primary_dependencies: &[#(#primary_dependency_type_ids),*],
                name: #final_component_name,
                condition: #condition_impl,
                constructor: || {
                    let wrapper = ::simple_starter_core::ComponentWrapper::<#component_type>::new(
                        #create_fn_impl,
                        None, // Provider 模式下 Init 逻辑通常包含在 create 中
                        #destroy_fn_impl
                    );
                    Box::new(wrapper)
                }
            }
        }
    };

    // 7. 输出结果
    let output = quote! {
        #func
        #inventory_impl
    };

    output.into()
}

/// 解析 provider 宏参数
fn parse_provider_args(
    args: TokenStream,
) -> syn::Result<(Option<String>, Option<syn::Expr>, Option<syn::Expr>)> {
    let mut name = None;
    let mut destroy_method: Option<syn::Expr> = None;
    let mut condition: Option<syn::Expr> = None;

    if args.is_empty() {
        return Ok((None, None, None));
    }

    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            let value: LitStr = meta.value()?.parse()?;
            name = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("destroy_method") {
            // 直接解析为表达式，支持路径或闭包
            let expr: syn::Expr = meta.value()?.parse()?;
            destroy_method = Some(expr);
            Ok(())
        } else if meta.path.is_ident("condition") {
            // 直接解析为表达式（如 ComponentCondition::on_missing_trait::<dyn X>()），嵌入惰性闭包
            let expr: syn::Expr = meta.value()?.parse()?;
            condition = Some(expr);
            Ok(())
        } else {
            Err(meta.error("unsupported property"))
        }
    });

    // 支持位置参数简写: #[provider("name")]
    if let Ok(lit) = syn::parse2::<LitStr>(args.clone().into()) {
        return Ok((Some(lit.value()), None, None));
    }

    // Key-Value 解析
    Parser::parse2(parser, args.clone().into())?;

    Ok((name, destroy_method, condition))
}