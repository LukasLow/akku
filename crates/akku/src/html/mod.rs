//! HTML as its own domain.
//!
//! `Html` is trusted markup. Build it with [`Html::new`] or [`Html::from`] and
//! let the HTTP layer turn it into a response. Text that must be escaped goes
//! through [`escape`].

use std::fmt;

/// Trusted HTML markup.
///
/// Constructing `Html` is an explicit statement that the string is already
/// valid, trusted markup. It is not escaped again by the HTTP layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Html(String);

impl Html {
    /// Create trusted HTML from a string.
    pub fn new(body: impl Into<String>) -> Self {
        Self(body.into())
    }

    /// Borrow the markup.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the markup.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<&str> for Html {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Html {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for Html {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Escape text for use in HTML text and quoted attribute values.
///
/// The replacements are `&`, `<`, `>`, `"` and `'`.
pub fn escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&#34;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
    output
}

#[cfg(feature = "http")]
impl axum::response::IntoResponse for Html {
    fn into_response(self) -> axum::response::Response {
        (
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            self.0,
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{escape, Html};

    #[test]
    fn escapes_the_five_html_characters() {
        assert_eq!(
            escape(r#"a & b < c > d "e" 'f'"#),
            "a &amp; b &lt; c &gt; d &#34;e&#34; &#39;f&#39;"
        );
    }

    #[test]
    fn html_is_trusted_and_keeps_its_markup() {
        let html = Html::new("<h1>Hello</h1>");
        assert_eq!(html.as_str(), "<h1>Hello</h1>");
        assert_eq!(html.to_string(), "<h1>Hello</h1>");
    }
}
