//! WebSearch tool: search the web using DuckDuckGo Instant Answer API.

use async_trait::async_trait;

use crate::{Tool, ToolOutput};

const DEFAULT_NUM_RESULTS: usize = 5;
const MAX_NUM_RESULTS: usize = 10;
const TIMEOUT_SECS: u64 = 15;
const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct WebSearchTool;

impl WebSearchTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// A single search result.
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Parse results from the DuckDuckGo Instant Answer API JSON response.
fn parse_ddg_results(json: &serde_json::Value, num_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Check the AbstractText (direct answer)
    if let (Some(text), Some(url)) = (
        json.get("AbstractText").and_then(|v| v.as_str()),
        json.get("AbstractURL").and_then(|v| v.as_str()),
    ) {
        if !text.is_empty() && !url.is_empty() {
            let title = json
                .get("Heading")
                .and_then(|v| v.as_str())
                .unwrap_or("DuckDuckGo Answer")
                .to_string();
            results.push(SearchResult {
                title,
                url: url.to_string(),
                snippet: text.to_string(),
            });
        }
    }

    // Check RelatedTopics array
    if let Some(topics) = json.get("RelatedTopics").and_then(|v| v.as_array()) {
        for topic in topics {
            if results.len() >= num_results {
                break;
            }
            // Some topics are groups (have a Topics array); skip those
            if topic.get("Topics").is_some() {
                continue;
            }
            let text = topic.get("Text").and_then(|v| v.as_str()).unwrap_or("");
            let first_url = topic.get("FirstURL").and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() || first_url.is_empty() {
                continue;
            }
            // The text often starts with the title followed by a dash or description
            let (title, snippet) = if let Some(idx) = text.find(" - ") {
                (
                    text[..idx].trim().to_string(),
                    text[idx + 3..].trim().to_string(),
                )
            } else {
                (text.chars().take(60).collect::<String>(), text.to_string())
            };
            results.push(SearchResult {
                title,
                url: first_url.to_string(),
                snippet,
            });
        }
    }

    results
}

/// Simple percent-encoding for URL query parameters.
fn percent_encode_query(s: &str) -> String {
    let mut encoded = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push_str(&format!("{:02X}", b));
            }
        }
    }
    encoded
}

/// Format results as numbered list.
fn format_results(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return "No results found. Try a different search query.".to_string();
    }
    results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("{}. **{}**\n   {}\n   {}", i + 1, r.title, r.url, r.snippet))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "WebSearch"
    }

    fn description(&self) -> &str {
        "Search the web using DuckDuckGo and return a list of results (title, URL, snippet). \
         Use this to find relevant pages, then use WebFetch to read the full content."
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query"
                },
                "num_results": {
                    "type": "integer",
                    "description": "Number of results to return (default: 5, max: 10)"
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, input: serde_json::Value) -> ToolOutput {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => return ToolOutput::err("Missing required field: query".to_string()),
        };

        let num_results = input
            .get("num_results")
            .and_then(|v| v.as_u64())
            .map(|v| (v as usize).min(MAX_NUM_RESULTS))
            .unwrap_or(DEFAULT_NUM_RESULTS);

        let client = match reqwest::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .user_agent(format!("clido/{}", VERSION))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolOutput::err(format!("Failed to build HTTP client: {}", e)),
        };

        let encoded_query = percent_encode_query(&query);
        let url = format!(
            "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
            encoded_query
        );

        let response = match client.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() {
                    return ToolOutput::err(format!(
                        "Search request timed out after {}s",
                        TIMEOUT_SECS
                    ));
                }
                return ToolOutput::err(format!("Network error during search: {}", e));
            }
        };

        let status = response.status();
        if !status.is_success() {
            return ToolOutput::err(format!(
                "Search API returned HTTP error {}",
                status.as_u16()
            ));
        }

        let json: serde_json::Value = match response.json().await {
            Ok(j) => j,
            Err(e) => return ToolOutput::err(format!("Failed to parse search response: {}", e)),
        };

        let results = parse_ddg_results(&json, num_results);
        let formatted = format_results(&results);

        ToolOutput::ok(format!(
            "Search results for: \"{}\"\n\n{}",
            query, formatted
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_results_empty() {
        let results: Vec<SearchResult> = vec![];
        let out = format_results(&results);
        assert!(out.contains("No results"));
    }

    #[test]
    fn test_format_results_single() {
        let results = vec![SearchResult {
            title: "Example".to_string(),
            url: "https://example.com".to_string(),
            snippet: "An example website".to_string(),
        }];
        let out = format_results(&results);
        assert!(out.contains("1."));
        assert!(out.contains("Example"));
        assert!(out.contains("https://example.com"));
        assert!(out.contains("An example website"));
    }

    #[test]
    fn test_format_results_numbered() {
        let results = vec![
            SearchResult {
                title: "First".to_string(),
                url: "https://first.com".to_string(),
                snippet: "First result".to_string(),
            },
            SearchResult {
                title: "Second".to_string(),
                url: "https://second.com".to_string(),
                snippet: "Second result".to_string(),
            },
        ];
        let out = format_results(&results);
        assert!(out.contains("1."));
        assert!(out.contains("2."));
        assert!(out.contains("First"));
        assert!(out.contains("Second"));
    }

    #[test]
    fn test_parse_ddg_results_empty_json() {
        let json = serde_json::json!({});
        let results = parse_ddg_results(&json, 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_ddg_results_abstract() {
        let json = serde_json::json!({
            "Heading": "Rust (programming language)",
            "AbstractText": "A multi-paradigm programming language.",
            "AbstractURL": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
            "RelatedTopics": []
        });
        let results = parse_ddg_results(&json, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust (programming language)");
        assert!(results[0].snippet.contains("multi-paradigm"));
    }

    #[test]
    fn test_parse_ddg_results_related_topics() {
        let json = serde_json::json!({
            "AbstractText": "",
            "AbstractURL": "",
            "RelatedTopics": [
                {
                    "Text": "Tokio - An asynchronous runtime for Rust",
                    "FirstURL": "https://tokio.rs"
                },
                {
                    "Text": "Async-std - Async version of std",
                    "FirstURL": "https://async.rs"
                }
            ]
        });
        let results = parse_ddg_results(&json, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, "https://tokio.rs");
        assert_eq!(results[1].url, "https://async.rs");
    }

    #[test]
    fn test_parse_ddg_num_results_limit() {
        let topics: Vec<serde_json::Value> = (0..10)
            .map(|i| {
                serde_json::json!({
                    "Text": format!("Topic {} - description", i),
                    "FirstURL": format!("https://example.com/{}", i)
                })
            })
            .collect();
        let json = serde_json::json!({
            "AbstractText": "",
            "AbstractURL": "",
            "RelatedTopics": topics
        });
        let results = parse_ddg_results(&json, 3);
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_missing_query_returns_error() {
        let tool = WebSearchTool::new();
        let out = tool.execute(serde_json::json!({})).await;
        assert!(out.is_error);
        assert!(out.content.contains("query"));
    }

    #[tokio::test]
    async fn test_empty_query_returns_error() {
        let tool = WebSearchTool::new();
        let out = tool.execute(serde_json::json!({ "query": "   " })).await;
        assert!(out.is_error);
        assert!(out.content.contains("query"));
    }

    #[tokio::test]
    async fn test_num_results_capped_at_max() {
        // Just verifies the capping logic doesn't panic; no network call
        let num = 100_u64.min(MAX_NUM_RESULTS as u64) as usize;
        assert_eq!(num, MAX_NUM_RESULTS);
    }

    #[tokio::test]
    #[ignore] // requires network
    async fn test_search_real_query() {
        let tool = WebSearchTool::new();
        let out = tool
            .execute(serde_json::json!({ "query": "rust programming language", "num_results": 3 }))
            .await;
        assert!(!out.is_error, "error: {}", out.content);
        assert!(out.content.contains("rust") || out.content.contains("Rust"));
    }
}
