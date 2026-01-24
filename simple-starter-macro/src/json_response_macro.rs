use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, ReturnType};
use syn::spanned::Spanned;

/// 实现 `#[json_response]` 宏的核心逻辑。
///
/// # 功能
/// 将普通的 `async fn() -> T` 转换为 axum 兼容的 handler `async fn() -> Json<T>`。
///
/// # 步骤
/// 1. 验证函数是否为 async。
/// 2. 提取原始返回类型 `T`。
/// 3. 生成包装函数，内部调用原逻辑并将结果包裹在 `Json()` 中。
pub(crate) fn json_response_macro(_args: TokenStream, input: TokenStream) -> TokenStream {
    // 1. 解析输入 AST
    let input_fn = parse_macro_input!(input as ItemFn);
    let sig = &input_fn.sig;
    let block = &input_fn.block;
    let attrs = &input_fn.attrs;
    let vis = &input_fn.vis;
    let func_name = &sig.ident;
    let inputs = &sig.inputs;

    // 2. 验证约束
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
            let span = sig.paren_token.span.span();
            return syn::Error::new(
                span,
                "json_response handler must have a return type"
            ).to_compile_error().into();
        }
        ReturnType::Type(_, ty) => ty,
    };

    // 3. 生成包装代码
    // 保持原有签名（可见性、参数），但修改返回类型为 Json<T>
    let expanded = quote! {
        #(#attrs)*
        #vis async fn #func_name(#inputs) -> simple_starter_web::axum::Json<#original_ret_ty> {
            // 在 async块中执行原逻辑
            let __result = async { #block }.await;
            // 自动包装
            simple_starter_web::axum::Json(__result)
        }
    };

    TokenStream::from(expanded)
}