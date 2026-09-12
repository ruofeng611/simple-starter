use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::ItemImpl;

/// 处理 `#[lifecycle]` 作用于 `impl ComponentLifecycle for Type` 块。
///
/// 验证 impl 的 trait 名为 `ComponentLifecycle`，生成 `LifecycleRegistration`
/// 提交到 inventory：accessor 将类型擦除的组件实例 safe downcast 到具体类型
/// 后正向 coercion 为 `Arc<dyn ComponentLifecycle>`（与 `#[injectable]` 的
/// accessor 同构，但直接返回 trait object，无需 vtable 拆解）。
///
/// 装配流程据此发现容器级周期参与者：全部 bean 完成注入与初始化后按创建
/// 顺序执行 `after_all_ready`，全部 bean 准备销毁之前按逆序批次执行
/// `before_destroy`（两方法均有默认空实现，按需覆写）。
pub(crate) fn lifecycle_on_impl(
    _args: TokenStream,
    item_impl: ItemImpl,
) -> syn::Result<TokenStream> {
    // 提取 trait 路径和实现类型，验证必须有 trait
    let impl_type = &item_impl.self_ty;
    let (_, trait_path, _) = item_impl.trait_.as_ref().ok_or_else(|| {
        syn::Error::new(
            item_impl.span(),
            "#[lifecycle] requires an impl block with a trait: 'impl ComponentLifecycle for Type'",
        )
    })?;

    // 验证 trait 名（仅支持 ComponentLifecycle）
    let trait_segment = trait_path.segments.last().ok_or_else(|| {
        syn::Error::new(trait_path.span(), "#[lifecycle] requires a trait path")
    })?;
    if trait_segment.ident != "ComponentLifecycle" {
        return Err(syn::Error::new(
            trait_segment.ident.span(),
            "#[lifecycle] only supports 'impl ComponentLifecycle for Type'",
        ));
    }

    // 生成 inventory 注册代码
    // accessor 使用非捕获闭包（自动 coerce 为 fn 指针），无需单独命名函数
    let registration = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_core::LifecycleRegistration {
                impl_type_id: ::std::any::TypeId::of::<#impl_type>(),
                accessor: |arc_any: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>|
                    -> ::std::option::Option<::std::sync::Arc<dyn ::simple_starter_core::ComponentLifecycle>>
                {
                    let arc: ::std::sync::Arc<#impl_type> = arc_any.downcast::<#impl_type>().ok()?;
                    ::std::option::Option::Some(
                        arc as ::std::sync::Arc<dyn ::simple_starter_core::ComponentLifecycle>
                    )
                },
            }
        }
    };

    let output = quote! {
        #item_impl
        #registration
    };

    Ok(output.into())
}
