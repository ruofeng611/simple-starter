use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::parse::Parser;
use syn::{Attribute, FnArg, ItemFn, LitStr, Meta, parse_macro_input};

/// 定时任务宏参数
pub(crate) struct CronArgs {
    /// cron 表达式默认值
    pub(crate) expr: Option<String>,
    /// 固定间隔默认值
    pub(crate) every: Option<String>,
    /// 任务名覆盖（缺省由宏按注册位置推导）
    pub(crate) name: Option<String>,
}

impl CronArgs {
    /// 校验参数组合：`expr` 与 `every` 互斥
    fn validate(&self, span: Span) -> syn::Result<()> {
        if self.expr.is_some() && self.every.is_some() {
            return Err(syn::Error::new(
                span,
                "`expr` and `every` are mutually exclusive; provide at most one",
            ));
        }
        Ok(())
    }

    /// 转换为 `Option<&'static str>` 字面量表达式
    fn literals(&self) -> (TokenStream2, TokenStream2) {
        let expr = option_literal(self.expr.as_deref());
        let every = option_literal(self.every.as_deref());
        (expr, every)
    }
}

/// 将可选字符串转为 `Some("...")` / `None` 表达式
fn option_literal(value: Option<&str>) -> TokenStream2 {
    match value {
        Some(value) => {
            let literal = LitStr::new(value, Span::call_site());
            quote! { Some(#literal) }
        }
        None => quote! { None },
    }
}

/// 解析定时任务宏参数
///
/// 支持三种写法：
/// - `#[cron_job]`：触发规则全部来自配置；
/// - `#[cron_job("*/5 * * * * *")]`：位置参数为 cron 表达式默认值；
/// - `#[cron_job(every = "30s", name = "...")]`：具名参数。
pub(crate) fn parse_cron_args(tokens: TokenStream2) -> syn::Result<CronArgs> {
    if tokens.is_empty() {
        return Ok(CronArgs {
            expr: None,
            every: None,
            name: None,
        });
    }

    // 位置参数简写：整个参数就是一个字符串字面量
    if let Ok(literal) = syn::parse2::<LitStr>(tokens.clone()) {
        return Ok(CronArgs {
            expr: Some(literal.value()),
            every: None,
            name: None,
        });
    }

    let mut expr = None;
    let mut every = None;
    let mut name = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("expr") {
            expr = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("every") {
            every = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse::<LitStr>()?.value());
            Ok(())
        } else {
            Err(meta.error("unsupported cron_job property; expected `expr`, `every` or `name`"))
        }
    });
    Parser::parse2(parser, tokens)?;

    let args = CronArgs { expr, every, name };
    args.validate(Span::call_site())?;
    Ok(args)
}

/// 从属性上解析定时任务宏参数
///
/// 供 `#[scheduled]` 读取并剥离方法上的 `#[cron_job]` 标记。
pub(crate) fn parse_cron_attr(attr: &Attribute) -> syn::Result<CronArgs> {
    match &attr.meta {
        // 裸标记：`#[cron_job]`
        Meta::Path(_) => Ok(CronArgs {
            expr: None,
            every: None,
            name: None,
        }),
        Meta::List(list) => parse_cron_args(list.tokens.clone()),
        Meta::NameValue(value) => Err(syn::Error::new_spanned(
            value,
            "`#[cron_job]` does not accept a direct value; use `#[cron_job(\"...\")]` or `#[cron_job(every = \"...\")]`",
        )),
    }
}

/// 生成任务注册代码
///
/// `factory` 为任务体构建闭包，需符合
/// `fn(&Arc<ComponentContainer>) -> Result<CronRunner, ComponentError>` 形态。
pub(crate) fn cron_job_registration(
    name: &str,
    args: &CronArgs,
    factory: TokenStream2,
) -> TokenStream2 {
    let name = LitStr::new(name, Span::call_site());
    let (expr, every) = args.literals();

    quote! {
        ::simple_starter_core::submit! {
            ::simple_starter_schedule::CronJob {
                name: #name,
                default_expr: #expr,
                default_every: #every,
                factory: #factory,
            }
        }
    }
}

/// 实现 `#[cron_job(...)]` 宏的核心逻辑。
///
/// 将自由 async 函数注册为定时任务：任务体在触发时调用该函数，
/// 不依赖组件实例。
///
/// # 步骤说明：
/// 1. 解析触发规则参数（cron 表达式 / 固定间隔 / 任务名）
/// 2. 校验函数为无参 `async fn`，且不是组件方法（方法形态由 `#[scheduled]` 处理）
/// 3. 生成构建闭包并注册 `CronJob`
pub(crate) fn cron_job_macro(args: TokenStream, item: TokenStream) -> TokenStream {
    let args = match parse_cron_args(args.into()) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };

    let func = parse_macro_input!(item as ItemFn);

    if let Some(FnArg::Receiver(receiver)) = func.sig.inputs.first() {
        return syn::Error::new_spanned(
            receiver,
            "`#[cron_job]` on a method requires `#[scheduled]` on the enclosing impl block",
        )
        .to_compile_error()
        .into();
    }

    if func.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            &func.sig.fn_token,
            "`#[cron_job]` can only be used on `async fn`",
        )
        .to_compile_error()
        .into();
    }

    if !func.sig.inputs.is_empty() {
        return syn::Error::new_spanned(
            &func.sig.inputs,
            "`#[cron_job]` on a free function requires no parameters; use `#[scheduled]` on an impl block for component methods",
        )
        .to_compile_error()
        .into();
    }

    let func_name = &func.sig.ident;
    let task_name = args
        .name
        .clone()
        .unwrap_or_else(|| func_name.to_string());

    let factory = quote! {
        |_container: &::std::sync::Arc<::simple_starter_core::ComponentContainer>| {
            Ok(Box::new(|| {
                Box::pin(async move {
                    #func_name().await
                })
            }))
        }
    };

    let registration = cron_job_registration(&task_name, &args, factory);

    quote! {
        #func
        #registration
    }
    .into()
}
