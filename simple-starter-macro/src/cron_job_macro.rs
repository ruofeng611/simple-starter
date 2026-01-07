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
/// 3. 生成一个同名 runner 函数，返回 `Pin<Box<dyn Future<Output=()> + Send>>`
/// 4. 使用 `submit!` 注册 `CronJob` 实例
pub(crate) fn cron_job_macro(args: TokenStream, item: TokenStream) -> TokenStream {
    // Step 1: 解析 cron 表达式（必须是字符串字面量）
    let cron_expr = parse_macro_input!(args as LitStr);

    // Step 2: 解析被修饰的函数
    let func = parse_macro_input!(item as ItemFn);

    // Step 3: 检查是否为 async 函数（非 async 报错）
    if func.sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            &func.sig.fn_token,
            "`#[cron_job]` can only be used on `async fn`",
        )
        .to_compile_error()
        .into();
    }

    // Step 4: 获取原函数名和 cron 表达式字符串
    let name = &func.sig.ident;
    let cron_expr_str = cron_expr.value();

    // Step 5: 生成扩展代码：
    //   - 保留原始 async 函数
    //   - 使用 **无捕获闭包** 作为 runner，直接调用原函数并返回 boxed future
    //   - 通过 `submit!` 宏注册到全局定时任务表
    let expanded = quote! {
        // 保留用户定义的原始 async 函数
        #func

        // 注册定时任务：使用闭包作为 runner
        ::simple_starter_core::submit!(
            ::simple_starter_core::CronJob {
                name: stringify!(#name),          // 任务名称（函数名）
                cron_expr: #cron_expr_str,        // cron 表达式（如 "0 0 * * *"）
                // runner: 无捕获闭包，等价于函数指针
                // 调用原 async 函数，并将其 future 包装为 Pin<Box<dyn Future>>
                runner: || {
                    ::std::boxed::Box::pin(async move {
                        #name().await
                    })
                },
            }
        );
    };

    TokenStream::from(expanded)
}
