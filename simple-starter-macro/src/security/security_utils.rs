use syn::LitStr;

// =============================================================================
// SecurityResourceArgs（用于 #[security_resource] 方法标记）
// =============================================================================

pub(crate) struct SecurityResourceArgs {
    pub(crate) resource_id: Option<String>,
    pub(crate) resource_name: Option<String>,
}

impl SecurityResourceArgs {
    pub(crate) fn parse_from_attr(attr: &syn::Attribute) -> Self {
        let mut resource_id = None;
        let mut resource_name = None;

        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("resource_id") {
                if let Ok(value) = meta.value()?.parse::<LitStr>() {
                    resource_id = Some(value.value());
                }
            } else if meta.path.is_ident("resource_name") {
                if let Ok(value) = meta.value()?.parse::<LitStr>() {
                    resource_name = Some(value.value());
                }
            }
            Ok(())
        });

        Self { resource_id, resource_name }
    }
}

// =============================================================================
// 工具函数
// =============================================================================

/// 从方法属性中查找 `#[security_resource(...)]` 并解析参数。
pub(crate) fn find_security_resource_attr(attrs: &[syn::Attribute]) -> Option<SecurityResourceArgs> {
    for attr in attrs {
        if attr.path().get_ident().map(|i| i == "security_resource").unwrap_or(false) {
            return Some(SecurityResourceArgs::parse_from_attr(attr));
        }
    }
    None
}

/// 从函数属性中提取 get/post/put/delete 的路径参数。
pub(crate) fn extract_route_path_from_fn_attrs(attrs: &[syn::Attribute]) -> String {
    for attr in attrs {
        let name = attr
            .path()
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        if matches!(name.as_str(), "get" | "post" | "put" | "delete") {
            return parse_route_macro_path(attr);
        }
    }
    "/".to_string()
}

/// 解析 `#[get("/path")]` 或 `#[get(path = "/path")]` 中的路径。
pub(crate) fn parse_route_macro_path(attr: &syn::Attribute) -> String {
    if let Ok(lit) = attr.parse_args::<LitStr>() {
        return lit.value();
    }

    let mut path = String::new();
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("path") {
            if let Ok(value) = meta.value()?.parse::<LitStr>() {
                path = value.value();
            }
        }
        Ok(())
    });

    if path.is_empty() {
        "/".to_string()
    } else {
        path
    }
}

/// 查找 mapping 属性并返回 (http_method, path)。
pub(crate) fn find_mapping_attr(attrs: &[syn::Attribute]) -> Option<(&str, String)> {
    for attr in attrs {
        let name = attr
            .path()
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        match name.as_str() {
            "get_mapping" => return Some(("get", parse_mapping_path(attr))),
            "post_mapping" => return Some(("post", parse_mapping_path(attr))),
            "put_mapping" => return Some(("put", parse_mapping_path(attr))),
            "delete_mapping" => return Some(("delete", parse_mapping_path(attr))),
            _ => {}
        }
    }
    None
}

/// 解析 mapping 宏的路径参数。
/// 支持 `#[get_mapping("/path")]` 或 `#[get_mapping(path = "/path")]`
pub(crate) fn parse_mapping_path(attr: &syn::Attribute) -> String {
    if let Ok(lit) = attr.parse_args::<LitStr>() {
        return lit.value();
    }

    let mut path = String::new();
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("path") {
            if let Ok(value) = meta.value()?.parse::<LitStr>() {
                path = value.value();
            }
        }
        Ok(())
    });

    if path.is_empty() {
        "/".to_string()
    } else {
        path
    }
}

/// 提取 rest_controller 的 base_path。
pub(crate) fn extract_rest_controller_path(attrs: &[syn::Attribute]) -> String {
    for attr in attrs {
        if attr
            .path()
            .get_ident()
            .map(|i| i == "rest_controller")
            .unwrap_or(false)
        {
            if let Ok(lit) = attr.parse_args::<LitStr>() {
                return lit.value();
            }
        }
    }
    "".to_string()
}

/// 从 impl self_ty 提取 controller 名称。
pub(crate) fn extract_controller_name(ty: &syn::Type) -> String {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident.to_string();
        }
    }
    "Controller".to_string()
}

/// 将路径参数从 `{id}` 格式转换为 Axum 的 `:id` 格式。
pub(crate) fn convert_path_params(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            // 跳过直到 }
            result.push(':');
            while let Some(inner) = chars.next() {
                if inner == '}' {
                    break;
                }
                result.push(inner);
            }
        } else {
            result.push(ch);
        }
    }

    result
}
