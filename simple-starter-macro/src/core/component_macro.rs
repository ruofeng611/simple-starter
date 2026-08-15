use crate::utils::macro_build_util::{
    get_arc_inner_type, get_dyn_trait_in_arc, get_dyn_trait_in_vec_arc,
    is_arc_dyn_trait, is_vec_arc_dyn_trait, parse_and_strip_inject,
    parse_and_strip_inject_primary, trait_object_to_type,
};
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::Parser;
use syn::{
    Data, DeriveInput, Ident, LitStr, parse_macro_input, spanned::Spanned,
};

/// `#[component]` 宏入口：仅作用于 struct。
///
/// 将结构体注册到依赖注入容器中，生成完整的生命周期管理代码
/// (create/init/destroy) 及依赖注入代码。
pub(crate) fn component_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    component_on_struct(args, input)
}

// =============================================================================
// #[component] on struct（含 trait object 注入支持）
// =============================================================================

/// 处理 `#[component]` 作用于 struct 的核心逻辑。
///
/// # 功能
/// 1. 解析组件配置（名称、初始化方法、销毁方法）。
/// 2. 扫描结构体字段，处理 `#[inject]` 依赖注入（含 trait object 支持）。
/// 3. 生成构造闭包（Constructor），自动组装依赖。
/// 4. 通过 `inventory` 注册组件元数据。
fn component_on_struct(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(input as DeriveInput);

    // 1. 解析宏参数
    let (component_name, init_method, destroy_method, condition) = match parse_component_args(args) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    let final_component_name = match component_name {
        Some(n) => n,
        None => ast.ident.to_string(),
    };

    // 2. 处理结构体字段与依赖注入
    let struct_name = &ast.ident;
    let mut field_injections = Vec::new();
    let mut dependencies_names = Vec::new();
    let mut trait_dependency_type_ids: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut type_dependency_type_ids: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut primary_dependency_type_ids: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut is_unit_struct = false;

    if let Data::Struct(ref mut data) = ast.data {
        if let syn::Fields::Named(fields) = &mut data.fields {
            for field in fields.named.iter_mut() {
                let (is_injected, inject_name) = parse_and_strip_inject(&mut field.attrs);
                let is_primary = parse_and_strip_inject_primary(&mut field.attrs);
                let field_ident = field.ident.as_ref().unwrap();

                // #[inject_primary] 单独使用即隐含注入语义；与 #[inject] 互斥
                if is_primary && is_injected {
                    return syn::Error::new(
                        field_ident.span(),
                        format!(
                            "#[inject_primary] cannot be combined with #[inject] on field '{}'",
                            field_ident
                        ),
                    )
                    .to_compile_error()
                    .into();
                }

                let is_injected = is_injected || is_primary;

                if !is_injected {
                    field_injections.push(quote! { #field_ident: Default::default() });
                    continue;
                }

                // ── 分支：根据字段类型选择注入策略 ──
                if is_primary {
                    // primary 注入：仅允许具体类型 Arc<T>（primary 按具体类型维度注册，
                    // 具体类型的 TypeId 才能命中 PRIMARY_BY_TYPE）
                    if is_vec_arc_dyn_trait(&field.ty) || is_arc_dyn_trait(&field.ty) {
                        return syn::Error::new(
                            field.ty.span(),
                            format!(
                                "#[inject_primary] on field '{}' requires a concrete type `Arc<T>`; trait types are not supported (primary is registered on the concrete type dimension)",
                                field_ident
                            ),
                        )
                        .to_compile_error()
                        .into();
                    }

                    let inner_type = match get_arc_inner_type(&field.ty) {
                        Some(ty) => ty,
                        None => {
                            return syn::Error::new(
                                field.ty.span(),
                                format!(
                                    "Field '{}' marked with #[inject_primary] must be of type Arc<T>",
                                    field_ident
                                ),
                            )
                            .to_compile_error()
                            .into();
                        }
                    };

                    primary_dependency_type_ids.push(quote! { ::std::any::TypeId::of::<#inner_type>() });
                    field_injections.push(quote! {
                        #field_ident: ::simple_starter_core::AppCoreUtil::get_primary_component::<#inner_type>()?
                    });
                    continue;
                }

                if is_vec_arc_dyn_trait(&field.ty) {
                    // Vec<Arc<dyn Trait>> → 收集所有实现
                    let trait_obj = get_dyn_trait_in_vec_arc(&field.ty).unwrap();
                    let trait_type = trait_object_to_type(trait_obj);
                    // 生成 TypeId 表达式（const fn，static 初始化中直接求值）。trait_type 已是 Type::TraitObject，无需再加 dyn 前缀
                    trait_dependency_type_ids.push(quote! { ::std::any::TypeId::of::<#trait_type>() });

                    let retrieval = quote! {
                        ::simple_starter_core::AppCoreUtil::get_components_by_trait::<#trait_type>()?
                    };
                    field_injections.push(quote! { #field_ident: #retrieval });

                } else if is_arc_dyn_trait(&field.ty) {
                    // Arc<dyn Trait> → 按 trait 获取唯一实现
                    let trait_obj = get_dyn_trait_in_arc(&field.ty).unwrap();
                    let trait_type = trait_object_to_type(trait_obj);

                    if let Some(name) = &inject_name {
                        // 按名称注入：依赖明确，直接加名称即可，
                        // 不需要 trait TypeId 依赖（避免对其他无关实现建立虚假依赖边）
                        dependencies_names.push(name.clone());
                        field_injections.push(quote! {
                            #field_ident: ::simple_starter_core::AppCoreUtil::get_component_by_trait_and_name::<#trait_type>(#name)?
                        });
                    } else {
                        // 按 trait 注入：依赖所有实现，通过 TypeId 解析
                        // trait_type 已是 Type::TraitObject，无需再加 dyn 前缀
                        trait_dependency_type_ids.push(quote! { ::std::any::TypeId::of::<#trait_type>() });
                        field_injections.push(quote! {
                            #field_ident: ::simple_starter_core::AppCoreUtil::get_component_by_trait::<#trait_type>()?
                        });
                    }

                } else {
                    // 普通类型 Arc<T>
                    let inner_type = match get_arc_inner_type(&field.ty) {
                        Some(ty) => ty,
                        None => {
                            return syn::Error::new(
                                field.ty.span(),
                                format!(
                                    "Field '{}' marked with #[inject] must be of type Arc<T>, Arc<dyn Trait>, or Vec<Arc<dyn Trait>>",
                                    field_ident
                                ),
                            )
                                .to_compile_error()
                                .into();
                        }
                    };

                    let retrieval_code = if let Some(name) = inject_name {
                        dependencies_names.push(name.clone());
                        quote! {
                            ::simple_starter_core::AppCoreUtil::get_component_by_name::<#inner_type, _>(#name)?
                        }
                    } else {
                        type_dependency_type_ids.push(quote! { ::std::any::TypeId::of::<#inner_type>() });
                        quote! {
                            ::simple_starter_core::AppCoreUtil::get_component::<#inner_type>()?
                        }
                    };
                    field_injections.push(quote! { #field_ident: #retrieval_code });
                }
            }
        } else if matches!(&data.fields, syn::Fields::Unit) {
            // 单元结构体（无字段）：无依赖注入，直接构造实例
            is_unit_struct = true;
        } else {
            return syn::Error::new(
                ast.span(),
                "#[component] only supports structs with named fields or unit structs",
            )
                .to_compile_error()
                .into();
        }
    } else {
        return syn::Error::new(ast.span(), "#[component] can only be used on structs")
            .to_compile_error()
            .into();
    }

    // 3. 生成生命周期包装函数
    let instance_construct = if is_unit_struct {
        quote! { let instance = #struct_name; }
    } else {
        quote! {
            let instance = #struct_name {
                #(#field_injections),*
            };
        }
    };
    let create_fn_impl = quote! {
        Box::new(move || -> ::simple_starter_core::BoxFuture<::simple_starter_core::anyhow::Result<#struct_name>> {
            Box::pin(async move {
                #instance_construct
                Ok(instance)
            })
        })
    };

    // init 闭包接收 Arc<T> 共享引用：对应用户方法签名 `async fn init(&self)`，
    // 实例所有权仍由组件仓库持有；destroy 闭包则接收所有权实例（见下方）
    let init_fn_impl = if let Some(method) = init_method {
        let method_ident = Ident::new(&method, Span::call_site());
        quote! {
            Some(Box::new(|c: std::sync::Arc<#struct_name>| -> ::simple_starter_core::BoxFuture<::simple_starter_core::anyhow::Result<()>> {
                Box::pin(async move {
                    let _ = c.#method_ident().await?;
                    Ok(())
                })
            }))
        }
    } else {
        quote! { None }
    };

    // destroy 闭包接收「有所有权」的实例 T：对应用户方法签名 `async fn destroy(self)`，
    // 销毁阶段所有权从仓库移出，用户方法可消费字段、取出内部资源（与 init 的 &self 不同）
    let destroy_fn_impl = if let Some(method) = destroy_method {
        let method_ident = Ident::new(&method, Span::call_site());
        quote! {
            Some(Box::new(|c: #struct_name| -> ::simple_starter_core::BoxFuture<::simple_starter_core::anyhow::Result<()>> {
                Box::pin(async move {
                    let _ = c.#method_ident().await?;
                    Ok(())
                })
            }))
        }
    } else {
        quote! { None }
    };

    // 4. 生成条件声明（条件表达式包进惰性闭包，注册期求值一次）
    let condition_impl = match condition {
        Some(expr) => quote! { Some(|| #expr) },
        None => quote! { None },
    };

    // 5. 生成 Inventory 注册代码
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
                    let wrapper = ::simple_starter_core::ComponentWrapper::<#struct_name>::new(
                        #create_fn_impl,
                        #init_fn_impl,
                        #destroy_fn_impl
                    );
                    Box::new(wrapper)
                }
            }
        }
    };

    let output = quote! {
        #ast
        #inventory_impl
    };

    output.into()
}

/// 解析组件宏参数
/// 支持格式：
/// - `#[component]`
/// - `#[component("name")]`
/// - `#[component(name="...", init_method="...", destroy_method="...", condition=...)]`
fn parse_component_args(
    args: TokenStream,
) -> syn::Result<(Option<String>, Option<String>, Option<String>, Option<syn::Expr>)> {
    let mut name = None;
    let mut init_method = None;
    let mut destroy_method = None;
    let mut condition = None;

    if args.is_empty() {
        return Ok((None, None, None, None));
    }

    // 定义 Key-Value 解析器
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            let value: LitStr = meta.value()?.parse()?;
            name = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("init_method") {
            let value: LitStr = meta.value()?.parse()?;
            init_method = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("destroy_method") {
            let value: LitStr = meta.value()?.parse()?;
            destroy_method = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("condition") {
            // 直接解析为表达式（如 ComponentCondition::on_missing_trait::<dyn X>()），嵌入惰性闭包
            let expr: syn::Expr = meta.value()?.parse()?;
            condition = Some(expr);
            Ok(())
        } else {
            Err(meta.error("unsupported component property"))
        }
    });

    // 尝试解析为单位置参数
    if let Ok(lit) = syn::parse2::<LitStr>(args.clone().into()) {
        return Ok((Some(lit.value()), None, None, None));
    }

    // 尝试解析为 Key-Value
    Parser::parse2(parser, args.clone().into())?;

    Ok((name, init_method, destroy_method, condition))
}
