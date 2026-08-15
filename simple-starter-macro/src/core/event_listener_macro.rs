use proc_macro::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{GenericArgument, ItemImpl, PathArguments};

/// 处理 `#[event_listener]` 作用于 `impl EventListener<E> for Type` 块。
///
/// 同时生成两个 inventory 注册：
///
/// 1. `TraitImplRegistration`：`impl EventListener<E> for Type` 的 trait 实现映射，
///    复用 trait 注入机制（`#[inject] Vec<Arc<dyn EventListener<E>>>` 可正常注入），
///    组件 create 后 `populate_trait_obj_cache` 填充 `TRAIT_OBJ_CACHE`；
/// 2. `EventListenerRegistration`：事件类型 ↔ 实现组件的监听器注册，
///    发布器 init 阶段遍历收集构建事件类型索引。
pub(crate) fn event_listener_on_impl(
    _args: TokenStream,
    item_impl: ItemImpl,
) -> syn::Result<TokenStream> {
    // 提取 trait 路径和实现类型，验证必须有 trait
    let impl_type = &item_impl.self_ty;
    let (_, trait_path, _) = item_impl.trait_.as_ref().ok_or_else(|| {
        syn::Error::new(
            item_impl.span(),
            "#[event_listener] requires an impl block with a trait: 'impl EventListener<E> for Type'",
        )
    })?;

    // 验证 trait 名并提取事件类型 E
    let event_type = extract_event_type(trait_path)?;

    // trait_path 转为 Type 才能用于 TypeId::of::<dyn Trait>()
    let trait_type: syn::Type = syn::Type::Path(syn::TypePath {
        qself: None,
        path: trait_path.clone(),
    });

    // 注册 1：trait 实现映射（与 #[injectable] 生成同款，先 coerce 到完整 trait vtable 再上转 Injectable）
    let trait_registration = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_core::TraitImplRegistration {
                trait_type_id: ::std::any::TypeId::of::<dyn #trait_type>(),
                impl_type_id: ::std::any::TypeId::of::<#impl_type>(),
                accessor: |arc_any: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>|
                    -> ::std::option::Option<::simple_starter_core::TraitObjectEntry>
                {
                    let arc: ::std::sync::Arc<#impl_type> = arc_any.downcast::<#impl_type>().ok()?;
                    // 先 coerce 到 dyn EventListener<E>（完整 vtable）
                    let arc_trait: ::std::sync::Arc<dyn #trait_type> = arc;
                    // 拆出 coercion 生成的 dyn EventListener<E> 真实 vtable（'static 只读静态数据），
                    // 取用侧按该 vtable 拼回 fat pointer，不依赖 vtable 布局假设。
                    let vtable = {
                        let raw = ::std::sync::Arc::into_raw(arc_trait.clone());
                        // SAFETY: fat pointer 位拆解（data + vtable 两段 usize），仅观察用途
                        let bits: [usize; 2] = unsafe { ::std::mem::transmute_copy(&raw) };
                        // SAFETY: 与 `Arc::into_raw` 配对收回，计数复原
                        let _ = unsafe { ::std::sync::Arc::from_raw(raw) };
                        bits[1] as *const ()
                    };
                    ::std::option::Option::Some(::simple_starter_core::TraitObjectEntry {
                        obj: arc_trait as ::std::sync::Arc<dyn ::simple_starter_core::Injectable>,
                        vtable,
                    })
                },
            }
        }
    };

    // 注册 2：事件监听器注册（发布器 init 收集用）
    let listener_registration = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_core::EventListenerRegistration {
                event_type_id: ::std::any::TypeId::of::<#event_type>(),
                listener_trait_type_id: ::std::any::TypeId::of::<dyn #trait_type>(),
                impl_type_id: ::std::any::TypeId::of::<#impl_type>(),
                adapter: |arc_injectable: ::std::sync::Arc<dyn ::simple_starter_core::Injectable>|
                    -> ::std::option::Option<::std::sync::Arc<dyn ::simple_starter_core::AnyEventListener>>
                {
                    // 还原链路全程 safe，不依赖 vtable 布局假设：
                    // 正向 upcast Injectable → Any（编译器生成 super_trait 槽偏移）
                    let arc_any: ::std::sync::Arc<dyn ::std::any::Any + Send + Sync> =
                        arc_injectable as ::std::sync::Arc<dyn ::std::any::Any + Send + Sync>;
                    // safe downcast Any → 具体实现类型（按 Any vtable 的 type_id 槽比对）
                    let arc_impl: ::std::sync::Arc<#impl_type> =
                        arc_any.downcast::<#impl_type>().ok()?;
                    // 正向 coercion 具体类型 → dyn EventListener<E>（完整 vtable）
                    let listener: ::std::sync::Arc<dyn #trait_type> = arc_impl;
                    ::std::option::Option::Some(::std::sync::Arc::new(
                        ::simple_starter_core::TypedListenerAdapter { inner: listener },
                    ))
                },
            }
        }
    };

    let output = quote! {
        #item_impl
        #trait_registration
        #listener_registration
    };

    Ok(output.into())
}

/// 从 trait 路径提取事件类型 E：
/// `impl EventListener<UserLoginEvent> for UserService` → `UserLoginEvent`
fn extract_event_type(trait_path: &syn::Path) -> syn::Result<syn::Type> {
    let segment = trait_path
        .segments
        .last()
        .ok_or_else(|| syn::Error::new(trait_path.span(), "#[event_listener] requires a trait path"))?;

    if segment.ident != "EventListener" {
        return Err(syn::Error::new(
            segment.ident.span(),
            "#[event_listener] only supports 'impl EventListener<E> for Type'",
        ));
    }

    match &segment.arguments {
        PathArguments::AngleBracketed(args) => {
            let event_arg = args.args.first().ok_or_else(|| {
                syn::Error::new(
                    args.span(),
                    "EventListener requires a type argument: 'impl EventListener<E> for Type'",
                )
            })?;
            match event_arg {
                GenericArgument::Type(ty) => Ok(ty.clone()),
                _ => Err(syn::Error::new(
                    event_arg.span(),
                    "EventListener type argument must be a concrete event type",
                )),
            }
        }
        _ => Err(syn::Error::new(
            segment.arguments.span(),
            "EventListener requires a type argument: 'impl EventListener<E> for Type'",
        )),
    }
}
