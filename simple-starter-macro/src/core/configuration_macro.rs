use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, LitStr, parse::Parser};

/// 实现 `#[configuration]` 宏
///
/// # 用途
/// 将一个结构体标记为配置组件，自动从全局配置文件(TOML)中读取对应路径的数据并反序列化。
///
/// # 参数
/// - 单参数简写: `#[configuration("server.http")]` -> prefix="server.http", name=结构体名
/// - 完整写法: `#[configuration(prefix = "server.http", name = "http_config")]`
pub(crate) fn configuration_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let struct_name = &ast.ident;

    // 1. 解析宏参数 (prefix, name, condition)
    let (prefix, custom_name, condition) = match parse_configuration_args(args) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    // 2. 确定组件最终注册名称（若未指定，默认为结构体名称）
    let final_component_name = custom_name.unwrap_or_else(|| struct_name.to_string());

    // 3. 生成构造闭包 (Constructor)
    // 逻辑：调用 AppCoreUtil::get_config_to_struct 读取配置
    // 注意：这要求目标结构体必须实现了 serde::Deserialize
    let create_fn_impl = quote! {
        Box::new(move || -> ::simple_starter_core::BoxFuture<::simple_starter_core::anyhow::Result<#struct_name>> {
            Box::pin(async move {
                // 尝试从配置路径加载
                let config = ::simple_starter_core::AppCoreUtil::get_config_to_struct::<#struct_name>(#prefix)?;
                Ok(config)
            })
        })
    };

    // 4. 生成条件声明（条件表达式包进惰性闭包，注册期求值一次）
    let condition_impl = match condition {
        Some(expr) => quote! { Some(|| #expr) },
        None => quote! { None },
    };

    // 5. 生成 Inventory 注册代码
    // 配置组件没有其他组件依赖，也没有 init/destroy 方法
    let inventory_impl = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_core::ComponentProcessorFactory {
                // 配置对象不需要依赖注入其他组件，所以依赖列表为空
                dependencies: &[],
                trait_dependencies: &[],
                type_dependencies: &[],
                primary_dependencies: &[],
                name: #final_component_name,
                condition: #condition_impl,
                constructor: || {
                    let wrapper = ::simple_starter_core::ComponentWrapper::<#struct_name>::new(
                        #create_fn_impl,
                        None, // 无初始化方法
                        None  // 无销毁方法
                    );
                    Box::new(wrapper)
                }
            }
        }
    };

    // 6. 输出最终代码
    let output = quote! {
        #ast
        #inventory_impl
    };

    output.into()
}

/// 解析配置宏参数
/// 返回 (Prefix, Option<Name>, Option<Condition>)
fn parse_configuration_args(
    args: TokenStream,
) -> syn::Result<(String, Option<String>, Option<syn::Expr>)> {
    let mut prefix = None;
    let mut name = None;
    let mut condition = None;

    if args.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[configuration] requires at least a 'prefix' argument",
        ));
    }

    // 1. 尝试解析为单字符串参数：#[configuration("my.config")]
    if let Ok(lit) = syn::parse2::<LitStr>(args.clone().into()) {
        return Ok((lit.value(), None, None));
    }

    // 2. 尝试解析为键值对：#[configuration(prefix = "...", name = "...", condition = <expr>)]
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("prefix") {
            let value: LitStr = meta.value()?.parse()?;
            prefix = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("name") {
            let value: LitStr = meta.value()?.parse()?;
            name = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("condition") {
            // 直接解析为表达式（如 ComponentCondition::on_missing_trait::<dyn X>()），嵌入惰性闭包
            let expr: syn::Expr = meta.value()?.parse()?;
            condition = Some(expr);
            Ok(())
        } else {
            Err(meta.error("unsupported configuration property. supported: 'prefix', 'name', 'condition'"))
        }
    });

    Parser::parse2(parser, args.clone().into())?;

    match prefix {
        Some(p) => Ok((p, name, condition)),
        None => Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "Missing required argument 'prefix' in #[configuration]",
        )),
    }
}