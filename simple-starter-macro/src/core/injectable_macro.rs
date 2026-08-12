use proc_macro::TokenStream;
use quote::quote;
use syn::ItemImpl;
use syn::spanned::Spanned;

/// 处理 `#[injectable]` 作用于 `impl Trait for Type` 块。
///
/// 验证 impl 必须实现某个 trait，生成 `TraitImplRegistration` 并提交到 inventory，
/// 将 trait ↔ 具体实现的映射关系注册到全局索引中。
pub(crate) fn injectable_on_impl(_args: TokenStream, item_impl: ItemImpl) -> syn::Result<TokenStream> {
    // 提取 trait 路径和实现类型，验证必须有 trait
    let impl_type = &item_impl.self_ty;
    let (_, trait_path, _) = item_impl
        .trait_
        .as_ref()
        .ok_or_else(|| {
            syn::Error::new(
                item_impl.span(),
                "#[injectable] requires an impl block with a trait: 'impl Trait for Type'",
            )
        })?;

    // trait_path 是 syn::Path（如 UserService 或 my_crate::UserService）
    // 需要转为 Type 才能用于 TypeId::of::<dyn Trait>()
    let trait_type: syn::Type = syn::Type::Path(syn::TypePath {
        qself: None,
        path: trait_path.clone(),
    });

    // 生成 inventory 注册代码
    // accessor 使用非捕获闭包（自动 coerce 为 fn 指针），无需单独命名函数
    let inventory_code = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_core::TraitImplRegistration {
                trait_type_id: ::std::any::TypeId::of::<dyn #trait_type>(),
                impl_type_id: ::std::any::TypeId::of::<#impl_type>(),
                accessor: |arc_any: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>|
                    -> ::std::option::Option<::std::sync::Arc<dyn ::simple_starter_core::Injectable>>
                {
                    let arc: ::std::sync::Arc<#impl_type> = arc_any.downcast::<#impl_type>().ok()?;
                    // 先 coerce 到 dyn Trait（完整 vtable），再向上 coerce 到 dyn Injectable
                    // 确保 vtable 基址 = Trait vtable，后续 transmute 回 Trait 才是正确的
                    let arc_trait: ::std::sync::Arc<dyn #trait_type> = arc;
                    ::std::option::Option::Some(
                        arc_trait as ::std::sync::Arc<dyn ::simple_starter_core::Injectable>,
                    )
                },
            }
        }
    };

    let output = quote! {
        #item_impl
        #inventory_code
    };

    Ok(output.into())
}
