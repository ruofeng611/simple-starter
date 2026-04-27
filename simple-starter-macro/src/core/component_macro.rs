use crate::utils::macro_build_util::{
    get_arc_inner_type, get_short_type_name_from_type, parse_and_strip_inject,
};
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::Parser;
use syn::{Data, DeriveInput, Ident, LitStr, parse_macro_input, spanned::Spanned};

/// 实现 `#[component]` 宏的核心逻辑。
///
/// # 功能
/// 1. 解析组件配置（名称、初始化方法、销毁方法）。
/// 2. 扫描结构体字段，处理 `#[inject]` 依赖注入。
/// 3. 生成构造闭包（Constructor），自动组装依赖。
/// 4. 通过 `inventory` 注册组件元数据。
pub(crate) fn component_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(input as DeriveInput);

    // 1. 解析宏参数
    // 例如：#[component(name = "auth", init_method = "start")]
    let (component_name, init_method, destroy_method) = match parse_component_args(args) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    // 确定组件最终注册名称（若未指定，默认为结构体名称）
    let final_component_name = match component_name {
        Some(n) => n,
        None => ast.ident.to_string(),
    };

    // 2. 处理结构体字段与依赖注入
    let struct_name = &ast.ident;
    let mut field_injections = Vec::new(); // 存储字段初始化代码
    let mut dependencies_names = Vec::new(); // 存储依赖项名称列表（用于拓扑排序）

    if let Data::Struct(ref mut data) = ast.data {
        if let syn::Fields::Named(ref mut fields) = data.fields {
            for field in fields.named.iter_mut() {
                // 解析并移除字段上的 #[inject] 属性
                let (is_injected, inject_name) = parse_and_strip_inject(&mut field.attrs);
                let field_ident = field.ident.as_ref().unwrap();

                if is_injected {
                    // 验证：被注入的字段必须是 Arc<T>
                    let inner_type = match get_arc_inner_type(&field.ty) {
                        Some(ty) => ty,
                        None => {
                            return syn::Error::new(
                                field.ty.span(),
                                format!(
                                    "Field '{}' marked with #[inject] must be of type Arc<T>",
                                    field_ident
                                ),
                            )
                                .to_compile_error()
                                .into();
                        }
                    };

                    // 生成获取依赖实例的代码
                    let retrieval_code = if let Some(name) = inject_name {
                        // 按名称获取
                        dependencies_names.push(name.clone());
                        quote! {
                             ::simple_starter_core::AppCoreUtil::get_component_by_name::<#inner_type, _>(#name)?
                        }
                    } else {
                        // 按类型获取
                        // 获取短类型名用于依赖关系记录
                        let short_type_name = get_short_type_name_from_type(inner_type);
                        dependencies_names.push(short_type_name);

                        quote! {
                             ::simple_starter_core::AppCoreUtil::get_component::<#inner_type>()?
                        }
                    };

                    field_injections.push(quote! {
                        #field_ident: #retrieval_code
                    });
                } else {
                    // 非注入字段，使用 Default 初始化
                    field_injections.push(quote! {
                        #field_ident: Default::default()
                    });
                }
            }
        } else {
            return syn::Error::new(
                ast.span(),
                "#[component] only supports structs with named fields",
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

    // 生成构造函数 (Constructor)
    // 返回一个 BoxFuture，内部创建结构体实例
    let create_fn_impl = quote! {
        Box::new(move || -> ::simple_starter_core::BoxFuture<::simple_starter_core::anyhow::Result<#struct_name>> {
            Box::pin(async move {
                let instance = #struct_name {
                    #(#field_injections),*
                };
                Ok(instance)
            })
        })
    };

    // 生成初始化函数 (Init)
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

    // 生成销毁函数 (Destroy)
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

    // 4. 生成 Inventory 注册代码
    // 将元数据注册到全局注册表中
    let inventory_impl = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_core::ComponentProcessorFactory {
                dependencies: &[#(#dependencies_names),*],
                name: #final_component_name,
                type_id: std::any::TypeId::of::<#struct_name>(),
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

    // 5. 输出最终代码
    // 包含：修改后的结构体定义（去除了 #[inject] 属性） + 注册代码
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
/// - `#[component(name="...", init_method="...", destroy_method="...")]`
fn parse_component_args(
    args: TokenStream,
) -> syn::Result<(Option<String>, Option<String>, Option<String>)> {
    let mut name = None;
    let mut init_method = None;
    let mut destroy_method = None;

    if args.is_empty() {
        return Ok((None, None, None));
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
        } else {
            Err(meta.error("unsupported component property"))
        }
    });

    // 尝试解析为单位置参数
    if let Ok(lit) = syn::parse2::<LitStr>(args.clone().into()) {
        return Ok((Some(lit.value()), None, None));
    }

    // 尝试解析为 Key-Value
    Parser::parse2(parser, args.clone().into())?;

    Ok((name, init_method, destroy_method))
}
