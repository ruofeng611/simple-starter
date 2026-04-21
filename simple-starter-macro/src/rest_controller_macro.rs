use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{parse_macro_input, Ident, ImplItem, ItemImpl, LitStr};

/// 实现 `#[rest_controller]` 宏的核心逻辑。
///
/// # 功能
/// 1. 解析基础路径参数（如 `#[rest_controller("/api")]`）。
/// 2. 扫描 impl 块中的所有方法，识别 `#[get_mapping]`、`#[post_mapping]` 等路由宏（只起标记作用，不移除）。
/// 3. 被 mapping 标记的方法：
///    - 返回值自动用 `Json<T>` 包裹。
///    - 原方法中的 Axum 提取器参数重写为裸类型（如 `Path(id): Path<i64>`  `id: i64`）。
///    - 生成对应的 Axum 路由 handler，参数原原本本保留提取器形式。
///    - handler 返回值同样声明为 `Json<T>`。
/// 4. 未被 mapping 标记的方法保持原样，不做任何修改。
pub(crate) fn rest_controller_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    // 1. 解析基础路径
    let base_path = if args.is_empty() {
        "".to_string()
    } else {
        match syn::parse2::<LitStr>(args.clone().into()) {
            Ok(lit) => lit.value(),
            Err(_) => {
                return syn::Error::new(
                    Span::call_site(),
                    "#[rest_controller] expects a string literal path, e.g., #[rest_controller(\"/api\")]",
                )
                .to_compile_error()
                .into();
            }
        }
    };

    // 2. 解析 impl 块
    let mut item_impl = parse_macro_input!(input as ItemImpl);
    let controller_type = &item_impl.self_ty;

    // 提取结构体名并转为 snake_case
    let struct_snake_name = extract_struct_snake_name(controller_type);

    // 3. 处理所有方法 + 收集路由信息
    let mut route_handlers = Vec::new();

    for item in &mut item_impl.items {
        if let ImplItem::Fn(method) = item {
            let method_name = method.sig.ident.clone();

            // 查找路由宏属性（只起标记作用，不移除）
            let mut route_info = None;
            for attr in method.attrs.iter() {
                let attr_name = attr
                    .path()
                    .get_ident()
                    .map(|i| i.to_string())
                    .unwrap_or_default();

                match attr_name.as_str() {
                    "get_mapping" | "post_mapping" | "put_mapping" | "delete_mapping" => {
                        let path = parse_mapping_path(attr);
                        let http_method = match attr_name.as_str() {
                            "get_mapping" => "get",
                            "post_mapping" => "post",
                            "put_mapping" => "put",
                            "delete_mapping" => "delete",
                            _ => unreachable!(),
                        };
                        route_info = Some((http_method, path));
                    }
                    _ => {}
                }
            }

            // 被 mapping 标记的方法：参数重写 + Json 包裹 + 生成 handler
            if let Some((http_method, method_path)) = route_info {
                // 记录原始参数（用于生成 handler）
                let original_inputs = method.sig.inputs.clone();

                // 提取器参数重写为裸类型
                rewrite_method_params(method);

                // 返回值 Json 化
                wrap_method_return_with_json(method);
                let full_path = combine_paths(&base_path, &method_path);
                let handler_fn_name = Ident::new(
                    &format!("{}_{}", struct_snake_name, method_name),
                    method_name.span(),
                );

                // 使用原始参数生成 handler 的参数列表
                let mut param_exprs = Vec::new();
                let mut axum_params = Vec::new();

                for (idx, input) in original_inputs.iter().enumerate() {
                    if idx == 0 {
                        continue; // 跳过 self
                    }
                    if let syn::FnArg::Typed(pat_type) = input {
                        let pat = &pat_type.pat;
                        let ty = &pat_type.ty;

                        // 原原本本放到 handler 中
                        axum_params.push(quote!(#pat: #ty));

                        // 生成调用表达式（提取内部值）
                        if let syn::Pat::TupleStruct(tuple_struct) = pat.as_ref() {
                            if let Some(first_elem) = tuple_struct.elems.first() {
                                if let syn::Pat::Ident(pat_ident) = first_elem {
                                    let ident = &pat_ident.ident;
                                    param_exprs.push(quote!(#ident));
                                } else {
                                    let param_name = Ident::new(&format!("p{}", idx), Span::call_site());
                                    param_exprs.push(quote!(#param_name));
                                }
                            } else {
                                let param_name = Ident::new(&format!("p{}", idx), Span::call_site());
                                param_exprs.push(quote!(#param_name));
                            }
                        } else if let syn::Pat::Ident(pat_ident) = pat.as_ref() {
                            let ident = &pat_ident.ident;
                            param_exprs.push(quote!(#ident));
                        } else {
                            let param_name = Ident::new(&format!("p{}", idx), Span::call_site());
                            param_exprs.push(quote!(#param_name));
                        }
                    }
                }

                // 获取返回类型（已经被 Json 化）
                let output = method.sig.output.clone();

                // 生成路由方法
                let router_method = match http_method {
                    "get" => quote! { simple_starter_web::axum::routing::get },
                    "post" => quote! { simple_starter_web::axum::routing::post },
                    "put" => quote! { simple_starter_web::axum::routing::put },
                    "delete" => quote! { simple_starter_web::axum::routing::delete },
                    _ => unreachable!(),
                };

                // 生成 handler 函数
                let handler_fn = if param_exprs.is_empty() {
                    quote! {
                        async fn #handler_fn_name(
                            simple_starter_web::axum::extract::State(controller): simple_starter_web::axum::extract::State<std::sync::Arc<#controller_type>>
                        ) #output {
                            controller.#method_name().await
                        }
                    }
                } else {
                    quote! {
                        async fn #handler_fn_name(
                            simple_starter_web::axum::extract::State(controller): simple_starter_web::axum::extract::State<std::sync::Arc<#controller_type>>,
                            #(#axum_params),*
                        ) #output {
                            controller.#method_name(#(#param_exprs),*).await
                        }
                    }
                };

                // 生成路由注册代码
                let route_registration = quote! {
                    #handler_fn

                    simple_starter_web::submit!(
                        simple_starter_web::RouteFactory {
                            router: || {
                                simple_starter_web::axum::Router::new()
                                    .route(#full_path, #router_method(#handler_fn_name))
                                    .with_state(
                                        simple_starter_core::AppCoreUtil::get_component::<#controller_type>()
                                            .expect(concat!("Failed to get ", stringify!(#controller_type), " component"))
                                    )
                            },
                        }
                    );
                };

                route_handlers.push(route_registration);
            }
        }
    }

    // 4. 生成最终代码
    let expanded = quote! {
        #item_impl

        #(#route_handlers)*
    };

    TokenStream::from(expanded)
}

/// 解析 mapping 宏的路径参数
/// 支持：#[get_mapping("/path")] 或 #[get_mapping(path = "/path")]
fn parse_mapping_path(attr: &syn::Attribute) -> String {
    if let Ok(lit) = attr.parse_args::<LitStr>() {
        return lit.value();
    }

    let mut path = String::new();
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("path") {
            let value: LitStr = meta.value()?.parse()?;
            path = value.value();
            Ok(())
        } else {
            Ok(())
        }
    });

    if path.is_empty() {
        "/".to_string()
    } else {
        path
    }
}

/// 组合基础路径和方法路径
fn combine_paths(base: &str, method: &str) -> String {
    let base = base.trim_matches('/');
    let method = method.trim_matches('/');

    if base.is_empty() && method.is_empty() {
        "/".to_string()
    } else if base.is_empty() {
        format!("/{}", method)
    } else if method.is_empty() {
        format!("/{}", base)
    } else {
        format!("/{}/{}", base, method)
    }
}

/// 尝试识别 Axum 提取器类型，并返回内部类型。
/// 支持的提取器：Path、Query、Json、Form
fn try_extract_extractor(ty: &syn::Type) -> Option<syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            let name = segment.ident.to_string();
            if matches!(name.as_str(), "Path" | "Query" | "Json" | "Form") {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                        return Some(inner_ty.clone());
                    }
                }
            }
        }
    }
    None
}

/// 尝试从任意单泛型类型中提取内部类型，不限名称。
/// 用于元组结构体解构模式（如 `Path(id): Path<i64>`），只要类型是 `Xxx<T>` 形式就解包。
fn try_extract_any_extractor(ty: &syn::Type) -> Option<syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                    return Some(inner_ty.clone());
                }
            }
        }
    }
    None
}

/// 重写 controller 方法的参数，将 Axum 提取器参数改为直接类型。
///
/// # 规则
/// - 元组结构体解构模式（如 `Path(id): Path<i64>`、`Header(name): Header<String>`）
///   明确表示提取器，不限名称，直接解包内部类型。
/// - 普通标识符模式（如 `payload: Json<CreateUser>`）需通过名称白名单确认是已知提取器。
///
/// 例如：
///   Path(id): Path<i64>          →  id: i64
///   Json(payload): Json<CreateUser>  →  payload: CreateUser
fn rewrite_method_params(method: &mut syn::ImplItemFn) {
    for input in method.sig.inputs.iter_mut() {
        if let syn::FnArg::Typed(pat_type) = input {
            match pat_type.pat.as_ref() {
                // 元组结构体模式如 Path(id): Path<i64>、Header(name): Header<String>
                // 这种解构写法明确就是提取器，不限名称，直接解包
                syn::Pat::TupleStruct(tuple_struct) => {
                    if let Some(inner_ty) = try_extract_any_extractor(&pat_type.ty) {
                        if let Some(first_elem) = tuple_struct.elems.first() {
                            if let syn::Pat::Ident(pat_ident) = first_elem {
                                let new_pat = syn::Pat::Ident(pat_ident.clone());
                                pat_type.pat = Box::new(new_pat);
                                pat_type.ty = Box::new(inner_ty);
                            }
                        }
                    }
                }
                // 普通标识符模式如 payload: Json<CreateUser>
                // 需要白名单确认，避免误伤用户自定义泛型类型
                syn::Pat::Ident(_) => {
                    if let Some(inner_ty) = try_extract_extractor(&pat_type.ty) {
                        pat_type.ty = Box::new(inner_ty);
                    }
                }
                _ => {}
            }
        }
    }
}

/// 将方法的返回值包装为 Json<T>。
/// 参考 json_response_macro 的实现：返回类型改为 Json<T>，方法体结果用 Json() 包裹。
fn wrap_method_return_with_json(method: &mut syn::ImplItemFn) {
    let original_ret_ty = match &method.sig.output {
        syn::ReturnType::Default => quote!(()),
        syn::ReturnType::Type(_, ty) => quote!(#ty),
    };

    // 修改返回类型为 Json<原始类型>
    method.sig.output = syn::parse2(quote! {
        -> simple_starter_web::axum::Json<#original_ret_ty>
    }).unwrap_or_else(|_| method.sig.output.clone());

    // 修改方法体
    let stmts = &method.block.stmts;
    method.block = syn::parse2(quote! {
        {
            let __result = async { #(#stmts)* }.await;
            simple_starter_web::axum::Json(__result)
        }
    }).unwrap_or_else(|_| method.block.clone());
}

/// 从类型中提取结构体名，并转为 snake_case。
/// 例如 `TestController` -> `test_controller`
fn extract_struct_snake_name(ty: &syn::Type) -> String {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return to_snake_case(&segment.ident.to_string());
        }
    }
    "controller".to_string()
}

/// 将 PascalCase / CamelCase 字符串转为 snake_case。
fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    let mut prev_lowercase = false;

    for (i, ch) in s.char_indices() {
        if ch.is_uppercase() {
            if i > 0 && prev_lowercase {
                result.push('_');
            }
            for lower in ch.to_lowercase() {
                result.push(lower);
            }
            prev_lowercase = true;
        } else {
            result.push(ch);
            prev_lowercase = true;
        }
    }

    result
}
