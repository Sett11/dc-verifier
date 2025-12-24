use dc_core::call_graph::HttpMethod;

/// Parses HTTP method string to HttpMethod enum
pub(crate) fn parse_http_method(method_str: &str) -> HttpMethod {
    match method_str.to_uppercase().as_str() {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        "HEAD" => HttpMethod::Head,
        "OPTIONS" => HttpMethod::Options,
        _ => HttpMethod::Get, // Default fallback
    }
}

