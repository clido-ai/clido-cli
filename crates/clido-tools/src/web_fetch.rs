//! WebFetch tool: fetch a URL and return its content as plain text.

use async_trait::async_trait;

use crate::{Tool, ToolOutput};

const DEFAULT_MAX_CHARS: usize = 20000;
const TIMEOUT_SECS: u64 = 30;
const MAX_REDIRECTS: usize = 5;
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct WebFetchTool;

impl WebFetchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip HTML tags and return plain text.
fn strip_html(html: &str) -> String {
    // Remove <script> blocks with their content (no backreferences in Rust regex).
    let re_script = regex::Regex::new(r"(?si)<script[^>]*>.*?</script>").unwrap();
    let no_scripts = re_script.replace_all(html, " ");
    // Remove <style> blocks with their content.
    let re_style = regex::Regex::new(r"(?si)<style[^>]*>.*?</style>").unwrap();
    let no_scripts = re_style.replace_all(&no_scripts, " ");

    // Remove all remaining HTML tags.
    let re_tags = regex::Regex::new(r"<[^>]+>").unwrap();
    let no_tags = re_tags.replace_all(&no_scripts, " ");

    // Decode common HTML entities.
    let decoded = no_tags
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&apos;", "'");

    // Collapse whitespace: replace runs of whitespace/newlines with a single space or newline.
    let re_ws = regex::Regex::new(r"[ \t]+").unwrap();
    let collapsed = re_ws.replace_all(&decoded, " ");
    let re_nl = regex::Regex::new(r"\n{3,}").unwrap();
    let collapsed = re_nl.replace_all(&collapsed, "\n\n");

    collapsed.trim().to_string()
}

/// Validate that the URL uses http or https scheme.
fn validate_url(url: &str) -> Result<(), String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!(
            "Invalid URL scheme. Only http:// and https:// are supported, got: {}",
            url
        ))
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "WebFetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL and return its content as plain text. Only HTTP and HTTPS URLs are supported. \
         Use this to read documentation, web pages, or any publicly accessible URL."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch (must be http:// or https://)"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum number of characters to return (default: 20000)"
                }
            },
            "required": ["url"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let url = match input.get("url").and_then(|v| v.as_str()) {
            Some(u) if !u.is_empty() => u.to_string(),
            _ => return ToolOutput::err("Missing required field: url".to_string()),
        };

        if let Err(e) = validate_url(&url) {
            return ToolOutput::err(e);
        }

        let max_chars = input
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_MAX_CHARS);

        let client = match reqwest::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .user_agent(format!("clido/{}", VERSION))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("Failed to build HTTP client: {}", e)),
        };

        let response = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() {
                    return ToolOutput::err(format!(
                        "Request timed out after {}s: {}",
                        TIMEOUT_SECS, url
                    ));
                }
                if e.is_redirect() {
                    return ToolOutput::err(format!(
                        "Too many redirects (max {}): {}",
                        MAX_REDIRECTS, url
                    ));
                }
                return ToolOutput::err(format!("Network error fetching {}: {}", url, e));
            }
        };

        let status = response.status();
        if !status.is_success() {
            return ToolOutput::err(format!("HTTP error {} fetching {}", status.as_u16(), url));
        }

        let body = match response.text().await {
            Ok(b) => b,
            Err(e) => return ToolOutput::err(format!("Failed to read response body: {}", e)),
        };

        let text = strip_html(&body);

        let (content, truncated) = if text.len() > max_chars {
            // Truncate at a newline boundary if possible.
            let cutoff = text[..max_chars].rfind('\n').unwrap_or(max_chars);
            let truncated_text = &text[..cutoff];
            (
                format!(
                    "{}\n\n[Truncated at {} chars — pass max_chars to increase limit]",
                    truncated_text, max_chars
                ),
                true,
            )
        } else {
            (text, false)
        };

        let _ = truncated; // used in format string above

        ToolOutput::ok(format!("Source: {}\n\n{}", url, content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_url_http() {
        assert!(validate_url("http://example.com").is_ok());
    }

    #[test]
    fn test_validate_url_https() {
        assert!(validate_url("https://example.com/path?q=1").is_ok());
    }

    #[test]
    fn test_validate_url_file_rejected() {
        assert!(validate_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_validate_url_ftp_rejected() {
        assert!(validate_url("ftp://example.com").is_err());
    }

    #[test]
    fn test_strip_html_basic() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = strip_html(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("<h1>"));
    }

    #[test]
    fn test_strip_html_removes_scripts() {
        let html = "<p>Content</p><script>alert('xss')</script><p>More</p>";
        let text = strip_html(html);
        assert!(text.contains("Content"));
        assert!(text.contains("More"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("xss"));
    }

    #[test]
    fn test_strip_html_decodes_entities() {
        let html = "<p>a &amp; b &lt; c &gt; d</p>";
        let text = strip_html(html);
        assert!(text.contains("a & b < c > d"));
    }

    #[tokio::test]
    async fn test_missing_url_returns_error() {
        let tool = WebFetchTool::new();
        let out = tool.execute(serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("url"));
    }

    #[tokio::test]
    async fn test_invalid_scheme_returns_error() {
        let tool = WebFetchTool::new();
        let out = tool
            .execute(serde_json::json!({ "url": "file:///etc/passwd" }))
            .await;
        assert!(out.is_error);
        assert!(out.content.contains("scheme") || out.content.contains("http"));
    }

    #[tokio::test]
    async fn test_ftp_scheme_returns_error() {
        let tool = WebFetchTool::new();
        let out = tool
            .execute(serde_json::json!({ "url": "ftp://example.com/file" }))
            .await;
        assert!(out.is_error);
    }

    #[tokio::test]
    #[ignore] // requires network
    async fn test_fetch_real_url() {
        let tool = WebFetchTool::new();
        let out = tool
            .execute(serde_json::json!({ "url": "https://example.com", "max_chars": 5000 }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert!(out.content.contains("example"), "content: {}", out.content);
    }
}
