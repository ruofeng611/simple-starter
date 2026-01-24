use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Expr, Ident, ItemFn, LitStr, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

/// 路由宏参数解析结构体。
///
/// 包含：
/// - `path`: 路由路径（必须）。
/// - `state_expr`: 状态构造表达式（可选，用于 `.with_state()`）。
struct RouteArgs {
    path: String,
    state_expr: Option<Expr>,
}

impl Parse for RouteArgs {
    /// 解析宏参数。
    ///
    /// # 支持格式
    /// 1. 位置参数：`"/path"`
    /// 2. 命名参数：`path = "/path", state = AppState::new()`
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut path = String::new();
        let mut state_expr = None;

        if input.is_empty() {
            return Err(input.error("expected path or named arguments"));
        }

        let lookahead = input.lookahead1();
        if lookahead.peek(LitStr) {
            // 情况1：位置参数 (仅路径)
            let lit: LitStr = input.parse()?;
            path = lit.value();
        } else {
            // 情况2：命名参数列表
            let args: Punctuated<NamedArg, Token![,]> =
                input.parse_terminated(NamedArg::parse, Token![,])?;
            for arg in args {
                match arg.name.to_string().as_str() {
                    "path" => {
                        if let Expr::Lit(syn::ExprLit {
                                             lit: syn::Lit::Str(s),
                                             ..
                                         }) = &arg.value
                        {
                            path = s.value();
                        } else {
                            return Err(syn::Error::new_spanned(
                                &arg.value,
                                "path must be a string literal",
                            ));
                        }
                    }
                    "state" => {
                        state_expr = Some(arg.value);
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &arg.name,
                            "unknown argument; expected `path` or `state`",
                        ));
                    }
                }
            }
        }

        if path.is_empty() {
            return Err(syn::Error::new_spanned(
                input.cursor().token_stream(),
                "missing path (either positional or `path = \"...\"`)",
            ));
        }

        Ok(RouteArgs { path, state_expr })
    }
}

/// 命名参数结构体 (key = value)。
struct NamedArg {
    name: Ident,
    value: Expr,
}

impl Parse for NamedArg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let value: Expr = input.parse()?;
        Ok(NamedArg { name, value })
    }
}

/// 路由宏的通用实现逻辑。
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
    let args = parse_macro_input!(args as RouteArgs);
    let input_fn = parse_macro_input!(input as ItemFn);
    let func_name = &input_fn.sig.ident;
    let path = &args.path;
    let state_expr = &args.state_expr;

    // 1. 映射方法名到 axum 路由构建器
    let router_method = match method {
        "get" => quote! { simple_starter_web::axum::routing::get },
        "post" => quote! { simple_starter_web::axum::routing::post },
        "put" => quote! { simple_starter_web::axum::routing::put },
        "delete" => quote! { simple_starter_web::axum::routing::delete },
        _ => {
            return syn::Error::new_spanned(
                &input_fn,
                format!("internal error: unsupported HTTP method `{}`", method),
            )
                .to_compile_error()
                .into();
        }
    };

    // 2. 构建 Router 初始化代码
    let router_build = if let Some(state) = state_expr {
        // 带状态注入
        quote! {
            simple_starter_web::axum::Router::new()
                .route(#path, #router_method(#func_name))
                .with_state(#state)
        }
    } else {
        // 无状态
        quote! {
            simple_starter_web::axum::Router::new()
                .route(#path, #router_method(#func_name))
        }
    };

    // 3. 最终展开：保留原函数 + 注册 RouteFactory
    let expanded = quote! {
        #input_fn
        simple_starter_web::submit!(
            simple_starter_web::RouteFactory {
                router: || { #router_build },
            }
        );
    };

    TokenStream::from(expanded)
}