//! Product rules for the browser's saved proxy candidates.

pub(super) const MAX_PROXY_OPTIONS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProxyOptionError {
    Invalid,
    Duplicate,
    Full,
}

pub(super) fn normalize_proxy_option(input: &str) -> Result<String, ProxyOptionError> {
    minicon_web::proxy_override(&minicon_web::BrowserProxy::default(), input)
        .map_err(|_| ProxyOptionError::Invalid)?
        .http
        .map(|endpoint| endpoint.to_ascii_lowercase())
        .ok_or(ProxyOptionError::Invalid)
}

pub(super) fn add_proxy_option(
    options: &mut Vec<String>,
    input: &str,
) -> Result<(), ProxyOptionError> {
    let endpoint = normalize_proxy_option(input)?;
    if options
        .iter()
        .any(|saved| normalize_proxy_option(saved).is_ok_and(|saved| saved == endpoint))
    {
        return Err(ProxyOptionError::Duplicate);
    }
    if options.len() >= MAX_PROXY_OPTIONS {
        return Err(ProxyOptionError::Full);
    }
    options.push(endpoint);
    Ok(())
}

/// A malformed saved endpoint must not prevent the rest of preferences loading.
/// Keep the first occurrence and order, up to the product's visible list limit.
pub(super) fn normalize_proxy_options(options: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for option in options {
        let _ = add_proxy_option(&mut normalized, &option);
        if normalized.len() == MAX_PROXY_OPTIONS {
            break;
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_use_the_shared_endpoint_validation_and_canonical_form() {
        assert_eq!(
            normalize_proxy_option("  http://LOCALHOST:08890/  ").unwrap(),
            "http://localhost:8890"
        );
        for invalid in [
            "",
            "  ",
            "https://localhost:8890",
            "http://localhost:0",
            "http://user@localhost:8890",
            "http://localhost:8890/path",
        ] {
            assert_eq!(
                normalize_proxy_option(invalid),
                Err(ProxyOptionError::Invalid)
            );
        }
    }

    #[test]
    fn adding_rejects_duplicates_and_full_lists_without_mutating_them() {
        let mut options = vec!["http://localhost:8890".to_owned()];
        assert_eq!(
            add_proxy_option(&mut options, "http://LOCALHOST:08890/"),
            Err(ProxyOptionError::Duplicate)
        );
        assert_eq!(
            add_proxy_option(&mut options, ""),
            Err(ProxyOptionError::Invalid)
        );
        assert_eq!(options, ["http://localhost:8890"]);
        for port in 8891..8898 {
            add_proxy_option(&mut options, &format!("http://localhost:{port}")).unwrap();
        }
        let before = options.clone();
        assert_eq!(
            add_proxy_option(&mut options, "http://localhost:8898"),
            Err(ProxyOptionError::Full)
        );
        assert_eq!(options, before);
        assert_eq!(options.len(), MAX_PROXY_OPTIONS);
    }

    #[test]
    fn loading_filters_invalid_duplicates_and_excess_preserving_first_order() {
        let mut input = vec![
            "invalid".to_owned(),
            "".to_owned(),
            "http://LOCALHOST:08890/".to_owned(),
            "http://localhost:8890".to_owned(),
        ];
        input.extend((8891..8905).map(|port| format!("http://localhost:{port}")));
        assert_eq!(
            normalize_proxy_options(input),
            (8890..8898)
                .map(|port| format!("http://localhost:{port}"))
                .collect::<Vec<_>>()
        );
        assert!(normalize_proxy_options(vec!["bad".to_owned()]).is_empty());
    }
}
