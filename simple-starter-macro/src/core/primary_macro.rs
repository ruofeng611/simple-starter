use crate::utils::macro_build_util::get_result_inner_type;
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::parse::Parser;
use syn::{parse_macro_input, spanned::Spanned, ItemFn, LitStr, ReturnType};

/// 处理 `#[primary]` 作用于 provider 函数。
///
/// 声明该函数返回值类型的"首要（primary）实例"：当框架按类型获取组件时优先返回它。
/// 必须与 `#[provider]` 一起标注在同一函数上（`#[primary]` 仅注册类型 → 实例名
/// 映射，不注册组件本身），且必须显式指定实例名，该名字必须与 `#[provider]`
/// 注册的组件名一致（启动注册期由 `component_loader` 校验存在性与同类型唯一性）。
pub(crate) fn primary_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    let func = parse_macro_input!(input as ItemFn);

    // 1. 解析实例名（必填：有 primary 说明一般有多个同类型实例，必须指名）
    let name = match parse_primary_args(args) {
        Ok(val) => val,
        Err(err) => return err.to_compile_error().into(),
    };

    // 2. 解析返回值类型 T（与 provider 相同的剥离 Result 逻辑）
    let fn_return_type = match &func.sig.output {
        ReturnType::Type(_, ty) => ty.as_ref().clone(),
        ReturnType::Default => {
            return syn::Error::new(
                func.sig.span(),
                "#[primary] requires a provider function that returns a type (e.g., anyhow::Result<T>)",
            )
            .to_compile_error()
            .into();
        }
    };
    let component_type = get_result_inner_type(&fn_return_type)
        .cloned()
        .unwrap_or(fn_return_type);

    // 3. 生成 PrimaryRegistration 注册代码（仅登记类型 → 实例名，组件本身由 #[provider] 注册）
    let inventory_code = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_core::PrimaryRegistration {
                type_id: ::std::any::TypeId::of::<#component_type>(),
                name: #name,
            }
        }
    };

    let output = quote! {
        #func
        #inventory_code
    };

    output.into()
}

/// 解析 primary 宏参数，支持两种形式：
/// - 位置参数简写: `#[primary("name")]`
/// - Key-Value: `#[primary(name = "name")]`
fn parse_primary_args(args: TokenStream) -> syn::Result<String> {
    if args.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "#[primary] requires an instance name: use #[primary(\"name\")] or #[primary(name = \"name\")]",
        ));
    }

    // 位置参数简写: #[primary("name")]
    if let Ok(lit) = syn::parse2::<LitStr>(args.clone().into()) {
        return Ok(lit.value());
    }

    // Key-Value: #[primary(name = "name")]
    let mut name = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            let value: LitStr = meta.value()?.parse()?;
            name = Some(value.value());
            Ok(())
        } else {
            Err(meta.error("unsupported property; #[primary] requires 'name'"))
        }
    });
    Parser::parse2(parser, args.clone().into())?;

    name.ok_or_else(|| {
        syn::Error::new(
            Span::call_site(),
            "#[primary] requires an instance name: use #[primary(\"name\")] or #[primary(name = \"name\")]",
        )
    })
}
