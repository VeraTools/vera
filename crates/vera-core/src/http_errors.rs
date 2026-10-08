//! Error text for remote model APIs that is safe to print.
//!
//! Endpoint URLs can carry private hostnames or keys in query strings, and
//! response bodies can echo credentials, so neither reaches user-facing text.

use std::error::Error as _;

const MAX_BODY_CHARS: usize = 500;

/// Describe a transport failure without its URL, including the underlying
/// cause (for example `connection refused`) that reqwest keeps in `source()`.
pub(crate) fn describe_transport_error(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.contains(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    redact_secrets(&text)
}

/// Bound an error response body to printable ASCII, cut at 500 characters,
/// with URLs and bearer tokens removed.
pub(crate) fn sanitize_error_body(body: &str) -> String {
    let truncated: String = body.chars().take(MAX_BODY_CHARS).collect();
    let printable = truncated.replace(|c: char| !c.is_ascii_graphic() && c != ' ', " ");
    let sanitized = redact_secrets(printable.trim());
    if sanitized.is_empty() {
        "no details available".to_string()
    } else {
        sanitized
    }
}

fn redact_secrets(text: &str) -> String {
    let mut redacted = Vec::new();
    let mut after_bearer = false;
    for word in text.split(' ') {
        redacted.push(if after_bearer && !word.is_empty() {
            "[redacted]"
        } else if word.contains("://") {
            "[url]"
        } else {
            word
        });
        after_bearer = word.eq_ignore_ascii_case("bearer");
    }
    redacted.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_drops_urls_tokens_and_control_characters() {
        let body = "bad key\tBearer sk-secret for (https://host.internal/v1?key=abc) retry";
        assert_eq!(
            sanitize_error_body(body),
            "bad key Bearer [redacted] for [url] retry"
        );
        assert_eq!(sanitize_error_body(" \n "), "no details available");
    }

    #[test]
    fn body_stays_within_500_bytes_at_multibyte_boundaries() {
        assert_eq!(sanitize_error_body(&"a".repeat(1000)).len(), 500);
        let sanitized = sanitize_error_body(&("a".repeat(499) + "🦀"));
        assert!(sanitized.len() <= 500);
    }

    #[tokio::test]
    async fn transport_error_names_the_cause_without_the_url() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/v1/embeddings?key=secret",
            listener.local_addr().unwrap()
        );
        drop(listener);

        crate::init_tls();
        let error = reqwest::Client::new().post(&url).send().await.unwrap_err();
        let text = describe_transport_error(error);

        assert!(
            !text.contains("127.0.0.1") && !text.contains("secret"),
            "{text}"
        );
        assert!(text.starts_with("error sending request: "), "{text}");
        assert!(text.to_lowercase().contains("connect"), "{text}");
    }
}
