use crate::security::security_utils::*;
use crate::utils::macro_build_util::combine_paths;
use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{ItemImpl, LitStr};
use syn::parse::Parser;

// =============================================================================
// SecurityControllerArgs（用于 #[security_controller] impl 块）
// =============================================================================

struct SecurityControllerArgs {
    module_id: Option<String>,
    module_name: Option<String>,
}

impl SecurityControllerArgs {
    fn parse(args: TokenStream) -> syn::Result<Self> {
        let mut module_id = None;
        let mut module_name = None;

        if !args.is_empty() {
            let parser = syn::meta::parser(|meta| {
                if meta.path.is_ident("module_id") {
                    let value: LitStr = meta.value()?.parse()?;
                    module_id = Some(value.value());
                    Ok(())
                } else if meta.path.is_ident("module_name") {
                    let value: LitStr = meta.value()?.parse()?;
                    module_name = Some(value.value());
                    Ok(())
                } else {
                    Err(meta.error("unsupported property; expected `module_id` or `module_name`"))
                }
            });
            parser.parse2(args.into())?;
        }

        Ok(Self { module_id, module_name })
    }
}

// =============================================================================
// #[security_controller] 主入口：仅支持 impl 块
// =============================================================================

pub(crate) fn security_controller_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = match SecurityControllerArgs::parse(args) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };

    // 解析为 impl 块
    let item_impl = match syn::parse::<ItemImpl>(input) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error().into(),
    };

    // 强制约束：必须搭配 #[rest_controller] 使用
    let has_rest_controller = item_impl.attrs.iter().any(|attr| {
        attr.path().get_ident().map(|i| i == "rest_controller").unwrap_or(false)
    });
    if !has_rest_controller {
        return syn::Error::new(
            Span::call_site(),
            "#[security_controller] must be placed above #[rest_controller] (attribute macros execute outer-to-inner)",
        )
        .to_compile_error()
        .into();
    }

    // 提取 controller 名称和 base_path
    let controller_name = extract_controller_name(&item_impl.self_ty);
    let base_path = extract_rest_controller_path(&item_impl.attrs);

    // 模块级默认值
    let module_id = args.module_id.unwrap_or_else(|| controller_name.clone());
    let module_name = args.module_name.unwrap_or_else(|| controller_name.clone());

    let mut registrations = Vec::new();

    for item in &item_impl.items {
        if let syn::ImplItem::Fn(method) = item {
            // 只处理有 mapping 标记的方法
            let Some((_, method_path)) = find_mapping_attr(&method.attrs) else {
                continue;
            };

            // 只处理显式标记了 #[security_resource] 的方法
            let Some(resource_args) = find_security_resource_attr(&method.attrs) else {
                continue;
            };

            let full_path = combine_paths(&base_path, &method_path);
            let method_name = method.sig.ident.to_string();

            let default_resource = format!("{}::{}", controller_name, method_name);
            let resource_id = resource_args.resource_id.unwrap_or_else(|| default_resource.clone());
            let resource_name = resource_args.resource_name.unwrap_or(default_resource);

            registrations.push(quote! {
                ::simple_starter_core::submit! {
                    ::simple_starter_security::ResourceEntry {
                        path_pattern: #full_path,
                        resource_id: #resource_id,
                        resource_name: #resource_name,
                        module_id: #module_id,
                        module_name: #module_name,
                    }
                }
            });
        }
    }

    let expanded = quote! {
        #(#registrations)*
        #item_impl
    };

    expanded.into()
}
