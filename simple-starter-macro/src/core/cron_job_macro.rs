use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, LitStr, parse_macro_input};

/// 实现 `#[cron_job(...)]` 宏的核心逻辑。
///
/// 将一个 async 函数包装成可被调度器调用的形式，并通过 `submit!` 宏注册为 CronJob。
///
/// # 步骤说明：
/// 1. 解析 cron 表达式（字符串字面量）
/// 2. 确保目标函数是 `async`
/// 3. 生成一个 runner 闭包，返回 `Pin<Box<dyn Future<Output=()> + Send>>`
/// 4. 使用 `submit!` 注册 `CronJob` 实例
pub(crate) fn cron_job_macro(args: TokenStream, item: TokenStream) -> TokenStream {
    // 1. 解析 cron 表达式（必须是字符串字面量）
    let cron_expr = parse_macro_input!(args as LitStr);

    // 2. 解析被修饰的函数 AST
    let func = parse_macro_input!(item as ItemFn);

    // 3. 验证是否为 async 函数
    if func.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            &func.sig.fn_token,
            "`#[cron_job]` can only be used on `async fn`",
        )
            .to_compile_error()
            .into();
    }

    // 4. 准备代码生成所需的变量
    let name = &func.sig.ident;
    let cron_expr_str = cron_expr.value();

    // 5. 生成扩展代码
    let expanded = quote! {
        // 保留用户定义的原始 async 函数
        #func

        // 注册定时任务
        ::simple_starter_core::submit!(
            ::simple_starter_core::CronJob {
                name: stringify!(#name),          // 任务名使用函数名
                cron_expr: #cron_expr_str,        // cron 表达式
                // runner: 构造一个无捕获闭包，适配调度器接口
                runner: || {
                    Box::pin(async move {
                        #name().await
                    })
                },
            }
        );
    };

    TokenStream::from(expanded)
}
