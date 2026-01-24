use proc_macro::TokenStream;

/// `#[inject]` 宏的空实现。
///
/// # 原理
/// 1. 此宏不做任何 AST 修改，直接返回原 TokenStream。
/// 2. 实际的注入逻辑由 `#[component]` 或 `#[provider]` 宏在解析字段/参数时主动处理。
/// 3. 此定义的存在是为了让 Rust 编译器识别 `#[inject]` 属性，避免产生 "unknown attribute" 错误。
pub(crate) fn inject_macro(_args: TokenStream, item: TokenStream) -> TokenStream {
    item
}