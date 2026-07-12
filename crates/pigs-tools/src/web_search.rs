//! Web search tool using DuckDuckGo Instant Answer API.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use pigs_core::{ToolError, ToolHandler, ToolResult, ToolSpec};
use serde::Deserialize;

/// Tool for lightweight web search / instant answers.
pub struct WebSearchTool {
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent("pigs-agent/0.1 (+https://github.com/local/pigs)")
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct DdgResponse {
    #[serde(default, rename = "AbstractText")]
    abstract_text: String,
    #[serde(default, rename = "AbstractURL")]
    abstract_url: String,
    #[serde(default, rename = "Heading")]
    heading: String,
    #[serde(default, rename = "Answer")]
    answer: String,
    #[serde(default, rename = "RelatedTopics")]
    related_topics: Vec<DdgRelated>,
}

#[derive(Debug, Deserialize)]
struct DdgRelated {
    #[serde(default, rename = "Text")]
    text: String,
    #[serde(default, rename = "FirstURL")]
    first_url: String,
    #[serde(default, rename = "Topics")]
    topics: Vec<DdgRelated>,
}

impl ToolHandler for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "web_search",
            "Search the web for a query using DuckDuckGo Instant Answer API. \
             Good for quick factual lookups, definitions, and related links. \
             Not a full SERP crawler.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum related results to include (1-10). Default: 5.",
                        "default": 5
                    }
                },
                "required": ["query"]
            }),
        )
    }

    fn execute<'a>(
        &'a self,
        input: serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            let query = input
                .get("query")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ToolError::InvalidInput("missing non-empty 'query'".into()))?;

            let max_results = input
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(5)
                .clamp(1, 10) as usize;

            let resp = self
                .client
                .get("https://api.duckduckgo.com/")
                .query(&[
                    ("q", query),
                    ("format", "json"),
                    ("no_html", "1"),
                    ("skip_disambig", "1"),
                ])
                .send()
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("web_search request failed: {e}")))?;

            if !resp.status().is_success() {
                return Ok(ToolResult::error(format!(
                    "web_search HTTP {}",
                    resp.status()
                )));
            }

            let data: DdgResponse = resp
                .json()
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("invalid web_search JSON: {e}")))?;

            let mut out = String::new();
            out.push_str(&format!("Query: {query}\n\n"));

            if !data.heading.is_empty() {
                out.push_str(&format!("Heading: {}\n", data.heading));
            }
            if !data.answer.is_empty() {
                out.push_str(&format!("Answer: {}\n", data.answer));
            }
            if !data.abstract_text.is_empty() {
                out.push_str(&format!("Abstract: {}\n", data.abstract_text));
                if !data.abstract_url.is_empty() {
                    out.push_str(&format!("Source: {}\n", data.abstract_url));
                }
            }

            let mut related = Vec::new();
            flatten_related(&data.related_topics, &mut related);
            if !related.is_empty() {
                out.push_str("\nRelated:\n");
                for (i, (text, url)) in related.into_iter().take(max_results).enumerate() {
                    if url.is_empty() {
                        out.push_str(&format!("{}. {}\n", i + 1, text));
                    } else {
                        out.push_str(&format!("{}. {} ({})\n", i + 1, text, url));
                    }
                }
            }

            if out.trim() == format!("Query: {query}") {
                out.push_str("\nNo instant answer found. Try a more specific query or use web_fetch on a known URL.\n");
            }

            Ok(ToolResult::success(out))
        })
    }
}

fn flatten_related(items: &[DdgRelated], out: &mut Vec<(String, String)>) {
    for item in items {
        if !item.text.is_empty() {
            out.push((item.text.clone(), item.first_url.clone()));
        }
        if !item.topics.is_empty() {
            flatten_related(&item.topics, out);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_tool_name_and_spec() {
        let tool = WebSearchTool::new();
        assert_eq!(tool.name(), "web_search");
        let spec = tool.spec();
        assert_eq!(spec.name, "web_search");
        assert!(spec.input_schema.get("properties").is_some());
    }

    #[test]
    fn test_flatten_related() {
        let items = vec![DdgRelated {
            text: "A".into(),
            first_url: "https://a.example".into(),
            topics: vec![DdgRelated {
                text: "B".into(),
                first_url: "https://b.example".into(),
                topics: vec![],
            }],
        }];
        let mut out = Vec::new();
        flatten_related(&items, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "A");
        assert_eq!(out[1].0, "B");
    }
}
