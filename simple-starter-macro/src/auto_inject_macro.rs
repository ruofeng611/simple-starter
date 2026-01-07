use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Ident, ItemStruct, LitStr, Meta, MetaList, Token, Type,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

/// 将驼峰命名转换为蛇形命名（用于生成方法名）
/// 例如：MyService → my_service
fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            result.push('_');
        }
        result.push(c.to_ascii_lowercase());
    }
    result
}

/// 解析 `#[auto_inject(types(...), names(...))]` 的参数
struct AutoInjectArgs {
    /// 要批量注入的类型列表，如 `types(Logger, Db)`
    types: Vec<Type>,
    /// 按名称注入的组件列表，如 `names(("cache", Cache))`
    names: Vec<(LitStr, Type)>,
}

/// 辅助结构：解析 `types(T1, T2, ...)` 中的类型列表
struct TypesList(pub Punctuated<Type, Token![,]>);

impl Parse for TypesList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(TypesList(Punctuated::parse_terminated(input)?))
    }
}

/// 表示 `names(("name", Type))` 中的一对
struct NameTypePair {
    name: LitStr, // 组件注册名
    ty: Type,     // 组件类型
}

impl Parse for NameTypePair {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let content;
        // 解析括号内的内容：("name", Type)
        syn::parenthesized!(content in input);
        let name = content.parse::<LitStr>()?;
        content.parse::<Token![,]>()?;
        let ty = content.parse::<Type>()?;
        Ok(NameTypePair { name, ty })
    }
}

/// 辅助结构：解析 `names((...), (...))` 列表
struct NamesList(pub Punctuated<NameTypePair, Token![,]>);

impl Parse for NamesList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(NamesList(Punctuated::parse_terminated(input)?))
    }
}

impl Parse for AutoInjectArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut types = Vec::new();
        let mut names = Vec::new();

        // 循环解析多个 MetaList，如 types(...) 和 names(...)
        while !input.is_empty() {
            let meta = input.parse::<Meta>()?;
            match meta {
                Meta::List(MetaList { path, tokens, .. }) => {
                    if path.is_ident("types") {
                        // 解析 types(T1, T2)
                        let list: TypesList = syn::parse2(tokens.clone())
                            .map_err(|e| syn::Error::new_spanned(&path, e))?;
                        types.extend(list.0.into_iter());
                    } else if path.is_ident("names") {
                        // 解析 names(("n1", T1), ("n2", T2))
                        let pairs: NamesList = syn::parse2(tokens.clone())
                            .map_err(|e| syn::Error::new_spanned(&path, e))?;
                        for pair in pairs.0 {
                            names.push((pair.name, pair.ty));
                        }
                    } else {
                        return Err(syn::Error::new_spanned(path, "expected `types` or `names`"));
                    }
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        meta,
                        "expected `types(...)` or `names(...)`",
                    ));
                }
            }
            // 如果还有内容，吃掉逗号（允许 args 间有逗号）
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(AutoInjectArgs { types, names })
    }
}

/// 从 Type 中提取最末尾的标识符（如 `MyService` from `Arc<MyService>` 不支持，仅支持简单路径）
fn extract_type_ident(ty: &Type) -> &Ident {
    match ty {
        Type::Path(p) => {
            &p.path
                .segments
                .last()
                .expect("type path must have at least one segment")
                .ident
        }
        _ => panic!("Expected a simple type name like `MyStruct`"),
    }
}

/// 实现 `#[auto_inject]` 宏：为结构体生成依赖获取方法
pub(crate) fn auto_inject_macro(args: TokenStream, input: TokenStream) -> TokenStream {
    // Step 1: 解析宏参数（types 和 names）
    let attr_args = syn::parse_macro_input!(args as AutoInjectArgs);

    // Step 2: 解析被修饰的结构体
    let input_struct = syn::parse_macro_input!(input as ItemStruct);
    let struct_name = &input_struct.ident;

    let mut methods = Vec::new();

    // Step 3: 为每个 `types(T)` 生成 `get_ts()` 方法（复数形式）
    for ty in &attr_args.types {
        let type_ident = extract_type_ident(ty);
        let method_name_str = format!("get_{}s", to_snake_case(&type_ident.to_string())); // 复数
        let method_name = Ident::new(&method_name_str, type_ident.span());
        methods.push(quote! {
            pub fn #method_name(&self) -> ::std::vec::Vec<::std::sync::Arc<::std::sync::RwLock<#ty>>> {
                ::simple_starter_core::AppCoreUtil::get_components_by_type::<#ty>().expect("Failed to get component")
            }
        });
    }

    // Step 4: 为每个 `names(("name", T))` 生成 `get_name()` 方法
    for (name_lit, ty) in &attr_args.names {
        let name_str = name_lit.value();
        let method_name_str = format!("get_{}", to_snake_case(&name_str));
        let method_name = Ident::new(&method_name_str, name_lit.span());
        methods.push(quote! {
            pub fn #method_name(&self) -> ::std::sync::Arc<::std::sync::RwLock<#ty>> {
                ::simple_starter_core::AppCoreUtil::get_component_by_name::<#ty>(#name_lit).expect("Failed to get component")
            }
        });
    }

    // Step 5: 生成 impl 块，包含所有注入方法
    let expanded = quote! {
        #input_struct
        impl #struct_name {
            #(#methods)*
        }
    };

    TokenStream::from(expanded)
}
