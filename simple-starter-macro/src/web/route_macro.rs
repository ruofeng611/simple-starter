use proc_macro::TokenStream;
use quote::quote;
use syn::{Expr, ItemFn, LitStr, parse::Parser, parse_macro_input};

/// 解析路由宏参数
///
/// 返回类型：`syn::Result<(路径String, 可选的状态Expr)>`
fn parse_route_args(args: TokenStream) -> syn::Result<(String, Option<Expr>)> {
    if args.is_empty() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "requires at least a 'path' argument",
        ));
    }

    // 1. 尝试解析为单位置参数 (简写模式)
    // 示例: #[get("/user/list")]
    if let Ok(lit) = syn::parse2::<LitStr>(args.clone().into()) {
        return Ok((lit.value(), None));
    }

    // 2. 准备变量用于存储解析结果
    let mut path = None;
    let mut state = None;

    // 3. 定义 meta parser (键值对模式)
    // 示例: #[get(path = "/user/list", state = AppState::new())]
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("path") {
            let value: LitStr = meta.value()?.parse()?;
            path = Some(value.value());
            Ok(())
        } else if meta.path.is_ident("state") {
            // 直接解析为 Expr，支持函数调用、变量名等
            let value: Expr = meta.value()?.parse()?;
            state = Some(value);
            Ok(())
        } else {
            // 遇到未知参数报错
            Err(meta.error("unsupported property; expected `path` or `state`"))
        }
    });

    // 4. 执行解析
    parser.parse2(args.clone().into())?;

    // 5. 校验必填项
    // 如果 path 依然是 None，说明用户没传 path 参数
    let path = path.ok_or_else(|| {
        syn::Error::new(proc_macro2::Span::call_site(), "missing `path` argument")
    })?;

    Ok((path, state))
}

/// 路由宏的通用实现逻辑
///
/// # 参数
/// - `method`: HTTP 方法名 ("get", "post" 等)。
/// - `args`: 宏参数 TokenStream。
/// - `input`: 被修饰的函数 TokenStream。
///
/// # 逻辑
/// 1. 解析参数，确定路径和 State。
/// 2. 根据 `method` 选择对应的 axum 路由函数。
/// 3. 生成包含原始 handler 和 `RouteFactory` 注册代码的 Block。
pub(crate) fn route_macro(method: &str, args: TokenStream, input: TokenStream) -> TokenStream {
    // 1. 调用上面的解析函数
    let (path, state_expr) = match parse_route_args(args) {
        Ok(res) => res,
        Err(err) => return err.to_compile_error().into(),
    };

    // 2. 解析被修饰的函数
    let input_fn = parse_macro_input!(input as ItemFn);
    let func_name = &input_fn.sig.ident;

    // 3. 映射 HTTP 方法到 axum 的路由构建函数
    let router_method = match method {
        "get" => quote! { ::simple_starter_web::axum::routing::get },
        "post" => quote! { ::simple_starter_web::axum::routing::post },
        "put" => quote! { ::simple_starter_web::axum::routing::put },
        "delete" => quote! { ::simple_starter_web::axum::routing::delete },
        _ => {
            return syn::Error::new_spanned(
                &input_fn,
                format!("internal error: unsupported HTTP method `{}`", method),
            )
            .to_compile_error()
            .into();
        }
    };

    // 4. 构建 Router 初始化代码
    // 根据是否有 state_expr 生成不同的代码块
    let router_build = if let Some(state) = state_expr {
        // 带有 .with_state(...)
        quote! {
            ::simple_starter_web::axum::Router::new()
                .route(#path, #router_method(#func_name))
                .with_state(#state)
        }
    } else {
        // 无状态
        quote! {
            ::simple_starter_web::axum::Router::new()
                .route(#path, #router_method(#func_name))
        }
    };

    // 5. 最终代码展开
    // - 保留原始函数定义 (#input_fn)
    // - 使用 submit! 宏注册 RouteFactory
    let expanded = quote! {
        #input_fn
        ::simple_starter_web::submit!(
            ::simple_starter_web::RouteFactory {
                router: || { #router_build },
            }
        );
    };

    TokenStream::from(expanded)
}
