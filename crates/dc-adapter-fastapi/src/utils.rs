use anyhow::Result;
use dc_core::call_graph::HttpMethod;

/// Parses HTTP method string to HttpMethod enum
///
/// # Errors
/// Returns an error if the method string is not a recognized HTTP method
pub(crate) fn parse_http_method(method_str: &str) -> Result<HttpMethod, anyhow::Error> {
    match method_str.to_uppercase().as_str() {
        "GET" => Ok(HttpMethod::Get),
        "POST" => Ok(HttpMethod::Post),
        "PUT" => Ok(HttpMethod::Put),
        "PATCH" => Ok(HttpMethod::Patch),
        "DELETE" => Ok(HttpMethod::Delete),
        "HEAD" => Ok(HttpMethod::Head),
        "OPTIONS" => Ok(HttpMethod::Options),
        _ => anyhow::bail!("Unrecognized HTTP method: {}", method_str),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_http_method_invalid() {
        assert!(parse_http_method("INVALID").is_err());
    }

    #[test]
    fn test_parse_http_method_valid() {
        assert!(parse_http_method("GET").is_ok());
        assert!(parse_http_method("POST").is_ok());
        assert!(parse_http_method("PUT").is_ok());
        assert!(parse_http_method("PATCH").is_ok());
        assert!(parse_http_method("DELETE").is_ok());
        assert!(parse_http_method("HEAD").is_ok());
        assert!(parse_http_method("OPTIONS").is_ok());
    }
}
