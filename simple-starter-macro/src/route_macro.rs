//! 路由宏实现（如 `#[get("/path")]`）。
//!
//! 本模块提供过程宏支持，允许用户以声明式方式注册 axum 路由。
//! 支持两种语法：
//! - 位置参数：`#[get("/hello")]`
//! - 命名参数：`#[post(path = "/api", state = AppState::new())]`
//!
//! 宏展开后：
//! 1. 保留原始函数；
//! 2. 通过 `inventory::submit!` 注册一个 `RouteFactory`，用于插件初始化时自动收集。

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Expr, Ident, ItemFn, LitStr, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

/// 表示路由宏的解析结果。
///
/// 包含：
/// - `path`: 必须提供的路由路径（字符串字面量）；
/// - `state_expr`: 可选的状态构造表达式（用于 `.with_state(...)`）。
struct RouteArgs {
    path: String,
    state_expr: Option<Expr>,
}

impl Parse for RouteArgs {
    /// 解析宏括号内的内容，支持两种形式：
    /// 1. 单个字符串字面量：`"/test"`
    /// 2. 命名参数列表：`path = "...", state = expr`
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut path = String::new();
        let mut state_expr = None;

        if input.is_empty() {
            return Err(input.error("expected path or named arguments"));
        }

        let lookahead = input.lookahead1();
        if lookahead.peek(LitStr) {
            // 情况1：位置参数
            let lit: LitStr = input.parse()?;
            path = lit.value();
        } else {
            // 情况2：命名参数（如 path=..., state=...）
            let args: Punctuated<NamedArg, Token![,]> =
                input.parse_terminated(NamedArg::parse, Token![,])?;

            for arg in args {
                match arg.name.to_string().as_str() {
                    "path" => {
                        // path 必须是字符串字面量（禁止变量）
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
                        // state 可以是任意表达式（如函数调用）
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

/// 表示一个命名参数（如 `state = MyApp::build()`）。
struct NamedArg {
    name: Ident, // 参数名
    value: Expr, // 表达式值
}

impl Parse for NamedArg {
    /// 解析 `name = value` 形式的 token。
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let value: Expr = input.parse()?;
        Ok(NamedArg { name, value })
    }
}

/// 通用路由宏实现。
///
/// 根据 HTTP 方法（get/post/put/delete）生成对应的 axum 路由注册代码。
///
/// # 参数
/// - `method`: HTTP 方法名（小写）
/// - `args`: 宏参数（TokenStream）
/// - `input`: 被修饰的函数
///
/// # 生成内容
/// 1. 保留原始函数（作为 handler）；
/// 2. 生成 `inventory::submit!(RouteFactory { ... })`，用于自动注册。
pub(crate) fn route_macro(method: &str, args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as RouteArgs);
    let input_fn = parse_macro_input!(input as ItemFn);
    let func_name = &input_fn.sig.ident;
    let path = &args.path;
    let state_expr = &args.state_expr;

    // 映射方法名到 axum 路由构建器
    let router_method = match method {
        "get" => quote! { ::simple_starter_web::axum::routing::get },
        "post" => quote! { ::simple_starter_web::axum::routing::post },
        "put" => quote! { ::simple_starter_web::axum::routing::put },
        "delete" => quote! { ::simple_starter_web::axum::routing::delete },
        _ => {
            // 使用 input_fn 的 span 作为错误位置（最接近的合法位置）
            return syn::Error::new_spanned(
                &input_fn,
                format!("internal error: unsupported HTTP method `{}`", method)
            ).to_compile_error().into();
        }
    };

    // 构建 Router 初始化代码
    let router_build = if let Some(state) = state_expr {
        // 带状态：.with_state(...)
        quote! {
            ::simple_starter_web::axum::Router::new()
                .route(#path, #router_method(#func_name))
                .with_state(#state)
        }
    } else {
        // 无状态：普通路由
        quote! {
            ::simple_starter_web::axum::Router::new()
                .route(#path, #router_method(#func_name))
        }
    };

    // 最终展开：保留函数 + 提交 RouteFactory
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