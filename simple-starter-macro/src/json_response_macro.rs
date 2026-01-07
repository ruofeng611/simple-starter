//! `#[json_response]` 属性宏的实现。
//!
//! 该宏用于简化 axum handler 的 JSON 响应编写：
//! - 自动将 `async fn() -> T` 转换为 `async fn() -> Json<T>`
//! - 在函数体内自动包裹返回值为 `Json(result)`
//!
//! 此宏要求函数必须是 `async` 且具有非空返回类型。

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, ReturnType};
use syn::spanned::Spanned;

/// 实现 `#[json_response]` 宏的核心逻辑。
///
/// 步骤：
/// 1. 解析输入为函数 AST（ItemFn）；
/// 2. 验证函数是 async 且有返回类型；
/// 3. 提取原始返回类型 T；
/// 4. 生成新函数：签名变为 `-> Json<T>`，函数体包裹原逻辑并返回 `Json(__result)`。
pub(crate) fn json_response_macro(_args: TokenStream, input: TokenStream) -> TokenStream {
    // 解析输入 token 流为函数 AST
    let input_fn = parse_macro_input!(input as ItemFn);
    let sig = &input_fn.sig;
    let block = &input_fn.block;
    let attrs = &input_fn.attrs;
    let vis = &input_fn.vis;
    let func_name = &sig.ident;
    let inputs = &sig.inputs;

    // --- 验证约束 ---

    // 必须是 async 函数
    if !sig.asyncness.is_some() {
        return syn::Error::new_spanned(
            &sig.fn_token,
            "json_response handler must be async"
        ).to_compile_error().into();
    }

    // 必须有显式返回类型（不能是 ()）
    let original_ret_ty = match &sig.output {
        ReturnType::Default => {
            // 使用函数括号位置作为错误提示点（更直观）
            let span = sig.paren_token.span.span();
            return syn::Error::new(
                span,
                "json_response handler must have a return type"
            ).to_compile_error().into();
        }
        ReturnType::Type(_, ty) => ty,
    };

    // --- 生成新函数 ---

    // 构造新函数：返回类型为 `Json<OriginalType>`，函数体包裹原逻辑
    let expanded = quote! {
        // 保留原始属性和可见性
        #(#attrs)*
        #vis async fn #func_name(#inputs) -> ::simple_starter_web::axum::Json<#original_ret_ty> {
            // 执行原函数体（在 async 块中避免作用域污染）
            let __result = async { #block }.await;
            // 包装为 Json 响应
            ::simple_starter_web::axum::Json(__result)
        }
    };

    TokenStream::from(expanded)
}