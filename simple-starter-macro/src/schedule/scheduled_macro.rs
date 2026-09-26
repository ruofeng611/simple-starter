use crate::schedule::cron_job_macro::{cron_job_registration, parse_cron_attr};
use crate::utils::macro_build_util::{get_short_type_name_from_type, is_unit_type};
use proc_macro::TokenStream;
use quote::quote;
use syn::{Attribute, FnArg, ImplItem, ItemImpl, ReturnType, parse_macro_input};

/// 实现 `#[scheduled]` 宏的核心逻辑。
///
/// 作用于组件类型的 `impl` 块：扫描带 `#[cron_job(...)]` 标记的方法，
/// 为其生成定时任务注册代码，并把标记从方法上剥离。
///
/// # 步骤说明：
/// 1. 解析 impl 块并取出组件类型
/// 2. 逐个方法取出 `#[cron_job(...)]` 标记并剥离
/// 3. 校验方法签名（async、仅 `&self`、返回 `()`）
/// 4. 生成构建闭包（在构建期解析组件实例并捕获）并注册 `CronJob`
pub(crate) fn scheduled_macro(args: TokenStream, item: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "`#[scheduled]` takes no arguments; declare triggers on methods via `#[cron_job(...)]`",
        )
        .to_compile_error()
        .into();
    }

    let mut item_impl = parse_macro_input!(item as ItemImpl);
    let self_ty = item_impl.self_ty.clone();
    let type_name = get_short_type_name_from_type(&self_ty);

    let mut registrations = Vec::new();

    for impl_item in item_impl.items.iter_mut() {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };

        let marker = match take_cron_marker(&mut method.attrs) {
            Ok(Some(marker)) => marker,
            Ok(None) => continue,
            Err(err) => return err.to_compile_error().into(),
        };

        let args = match parse_cron_attr(&marker) {
            Ok(args) => args,
            Err(err) => return err.to_compile_error().into(),
        };

        if let Err(err) = validate_method(method) {
            return err.to_compile_error().into();
        }

        let method_name = &method.sig.ident;
        let task_name = args
            .name
            .clone()
            .unwrap_or_else(|| format!("{type_name}::{method_name}"));

        let factory = quote! {
            |container: &::std::sync::Arc<::simple_starter_core::ComponentContainer>| {
                let instance = container.get_component::<#self_ty>()?;
                Ok(Box::new(move || {
                    let instance = instance.clone();
                    Box::pin(async move {
                        instance.#method_name().await
                    })
                }))
            }
        };

        registrations.push(cron_job_registration(&task_name, &args, factory));
    }

    quote! {
        #item_impl

        #(#registrations)*
    }
    .into()
}

/// 取出并移除方法上的 `#[cron_job]` 标记
///
/// 标记被外层宏剥离，因此不会再由 `#[cron_job]` 自身展开。
fn take_cron_marker(attrs: &mut Vec<Attribute>) -> syn::Result<Option<Attribute>> {
    let mut marker = None;
    let mut retained = Vec::with_capacity(attrs.len());

    for attr in attrs.drain(..) {
        if attr.path().is_ident("cron_job") {
            if marker.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "duplicate `#[cron_job]` on the same method",
                ));
            }
            marker = Some(attr);
        } else {
            retained.push(attr);
        }
    }

    *attrs = retained;
    Ok(marker)
}

/// 校验待注册方法的签名：`async`、仅接收 `&self`、返回 `()`
fn validate_method(method: &syn::ImplItemFn) -> syn::Result<()> {
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &method.sig.fn_token,
            "`#[cron_job]` method must be `async`",
        ));
    }

    let mut inputs = method.sig.inputs.iter();
    match inputs.next() {
        Some(FnArg::Receiver(receiver))
            if receiver.reference.is_some() && receiver.mutability.is_none() => {}
        Some(other) => {
            return Err(syn::Error::new_spanned(
                other,
                "`#[cron_job]` method must take `&self` as its only parameter",
            ));
        }
        None => {
            return Err(syn::Error::new_spanned(
                &method.sig.ident,
                "`#[cron_job]` method must take `&self`",
            ));
        }
    }

    if let Some(extra) = inputs.next() {
        return Err(syn::Error::new_spanned(
            extra,
            "`#[cron_job]` method must not take parameters other than `&self`",
        ));
    }

    match &method.sig.output {
        ReturnType::Default => Ok(()),
        ReturnType::Type(_, ty) if is_unit_type(ty) => Ok(()),
        ReturnType::Type(_, ty) => Err(syn::Error::new_spanned(
            ty,
            "`#[cron_job]` method must return `()`; handle failures inside the method",
        )),
    }
}
