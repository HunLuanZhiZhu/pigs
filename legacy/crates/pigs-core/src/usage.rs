//! Token usage tracking.

use serde::{Deserialize, Serialize};

/// Token usage from an LLM API call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost: Option<f64>,
}

impl TokenUsage {
    /// Create a new usage record.
    pub fn new(input: u64, output: u64) -> Self {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: None,
            total_cost: None,
        }
    }

    /// Total tokens (input + output).
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Add another usage record to this one.
    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens = match (self.cache_read_tokens, other.cache_read_tokens) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        self.total_cost = match (self.total_cost, other.total_cost) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }

    /// Set cost estimate.
    pub fn with_cost(mut self, cost: f64) -> Self {
        self.total_cost = Some(cost);
        self
    }

    /// Estimate cost based on per-million-token rates.
    pub fn estimate_cost(&self, input_per_million: f64, output_per_million: f64) -> f64 {
        let input = self.input_tokens as f64 / 1_000_000.0 * input_per_million;
        let output = self.output_tokens as f64 / 1_000_000.0 * output_per_million;
        input + output
    }

    /// Estimate USD cost for a known model family using rough public list prices.
    /// Returns None when the model is unknown.
    pub fn estimate_cost_for_model(&self, model: &str) -> Option<f64> {
        let (input_rate, output_rate) = model_pricing_per_million(model)?;
        Some(self.estimate_cost(input_rate, output_rate))
    }
}

/// Rough USD pricing per 1M tokens for common model families.
/// These are approximate and intended for local cost awareness only.
pub fn model_pricing_per_million(model: &str) -> Option<(f64, f64)> {
    let m = model.to_lowercase();
    // Rough list prices for common Anthropic / OpenAI model name patterns only.
    // Other models (including OpenAI-compatible third parties) return None.
    if m.contains("claude-opus") || (m.contains("opus") && m.contains("claude")) {
        Some((15.0, 75.0))
    } else if m.contains("claude-sonnet") || m.contains("sonnet") {
        Some((3.0, 15.0))
    } else if m.contains("claude") && m.contains("haiku") {
        Some((0.80, 4.0))
    } else if m.contains("gpt-4o-mini") {
        Some((0.15, 0.60))
    } else if m.contains("gpt-4o") || m == "gpt-4" {
        Some((2.50, 10.0))
    } else if m.contains("o1") || m.contains("o3") {
        Some((15.0, 60.0))
    } else {
        None
    }
}

impl std::fmt::Display for TokenUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "input={}, output={}",
            self.input_tokens, self.output_tokens
        )?;
        if let Some(cached) = self.cache_read_tokens {
            write!(f, ", cached={cached}")?;
        }
        if let Some(cost) = self.total_cost {
            write!(f, ", cost=${cost:.4}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_usage_add() {
        let mut a = TokenUsage::new(100, 50);
        let b = TokenUsage::new(200, 150);
        a.add(&b);
        assert_eq!(a.input_tokens, 300);
        assert_eq!(a.output_tokens, 200);
    }

    #[test]
    fn test_cost_estimation() {
        let usage = TokenUsage::new(1_000_000, 500_000);
        let cost = usage.estimate_cost(3.0, 15.0);
        assert!((cost - 10.5).abs() < 0.001);
    }

    #[test]
    fn test_estimate_cost_for_model() {
        let usage = TokenUsage::new(1_000_000, 0);
        let cost = usage
            .estimate_cost_for_model("claude-sonnet-4-20250514")
            .unwrap();
        assert!((cost - 3.0).abs() < 0.001);
        let gpt = usage.estimate_cost_for_model("gpt-4o").unwrap();
        assert!((gpt - 2.50).abs() < 0.001);
        // Non-catalog models (including third-party OpenAI-compatible ids) have no built-in price.
        assert!(usage.estimate_cost_for_model("llama3.2").is_none());
        assert!(usage
            .estimate_cost_for_model("totally-unknown-model-xyz")
            .is_none());
    }
}
