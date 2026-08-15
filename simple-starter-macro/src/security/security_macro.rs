use crate::security::security_utils::*;
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{ItemFn, LitStr};
use syn::parse::Parser;

// =============================================================================
// SecurityArgs（用于 #[security] 自由函数）
// =============================================================================

struct SecurityArgs {
    resource_id: Option<String>,
    resource_name: Option<String>,
    module_id: Option<String>,
    module_name: Option<String>,
}

impl SecurityArgs {
    fn parse(args: TokenStream) -> syn::Result<Self> {
        let mut resource_id = None;
        let mut resource_name = None;
        let mut module_id = None;
        let mut module_name = None;

        if !args.is_empty() {
            let parser = syn::meta::parser(|meta| {
                if meta.path.is_ident("resource_id") {
                    let value: LitStr = meta.value()?.parse()?;
                    resource_id = Some(value.value());
                    Ok(())
                } else if meta.path.is_ident("resource_name") {
                    let value: LitStr = meta.value()?.parse()?;
                    resource_name = Some(value.value());
                    Ok(())
                } else if meta.path.is_ident("module_id") {
                    let value: LitStr = meta.value()?.parse()?;
                    module_id = Some(value.value());
                    Ok(())
                } else if meta.path.is_ident("module_name") {
                    let value: LitStr = meta.value()?.parse()?;
                    module_name = Some(value.value());
                    Ok(())
                } else {
                    Err(meta.error("unsupported property; expected `resource_id`, `resource_name`, `module_id`, or `module_name`"))
                }
            });
            parser.parse2(args.into())?;
        }

        Ok(Self {
            resource_id,
            resource_name,
            module_id,
            module_name,
        })
    }
}

// =============================================================================
// #[security] 主入口：仅支持自由函数
// =============================================================================

pub(crate) fn security_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match SecurityArgs::parse(args) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };

    // 只支持自由函数
    if let Ok(func) = syn::parse::<ItemFn>(input) {
        return security_on_fn(args, func);
    }

    syn::Error::new(
        Span::call_site(),
        "#[security] can only be applied to a function. For impl blocks, use #[security_controller].",
    )
    .to_compile_error()
    .into()
}

// =============================================================================
// 函数级处理（配合 get / post / put / delete）
// =============================================================================

fn security_on_fn(args: SecurityArgs, func: ItemFn) -> TokenStream {
    let func_name = func.sig.ident.to_string();

    // 强制约束：必须搭配 #[get] / #[post] / #[put] / #[delete] 之一使用
    let has_route_attr = func.attrs.iter().any(|attr| {
        let name = attr
            .path()
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        matches!(name.as_str(), "get" | "post" | "put" | "delete")
    });
    if !has_route_attr {
        return syn::Error::new(
            Span::call_site(),
            "#[security] on function must be placed above #[get], #[post], #[put], or #[delete] (attribute macros execute outer-to-inner)",
        )
        .to_compile_error()
        .into();
    }

    let path = extract_route_path_from_fn_attrs(&func.attrs);

    let resource_id = args
        .resource_id
        .unwrap_or_else(|| func_name.clone());
    let resource_name = args
        .resource_name
        .unwrap_or_else(|| func_name.clone());
    let module_id = args.module_id.unwrap_or_default();
    let module_name = args.module_name.unwrap_or_default();

    let register = quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_security::ResourceEntry {
                path_pattern: #path,
                resource_id: #resource_id,
                resource_name: #resource_name,
                module_id: #module_id,
                module_name: #module_name,
            }
        }
    };

    let expanded = quote! {
        #register
        #func
    };

    expanded.into()
}
