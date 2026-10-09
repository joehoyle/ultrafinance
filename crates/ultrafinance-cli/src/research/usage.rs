use serde::Serialize;
use serde_json::Value;

// Standard USD rates verified 2026-10-09:
// https://developers.openai.com/api/docs/pricing
// https://developers.openai.com/api/docs/guides/prompt-caching
const SEARCH_USD: f64 = 0.01;

#[derive(Clone, Debug, Serialize)]
pub(super) struct Usage {
    pub responses: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub web_searches: u64,
    pub estimated_cost_usd: Option<f64>,
    pub partial: bool,
}
impl Default for Usage {
    fn default() -> Self {
        Self {
            responses: 0,
            input_tokens: 0,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            web_searches: 0,
            estimated_cost_usd: Some(0.0),
            partial: false,
        }
    }
}
impl Usage {
    pub fn observe(&mut self, response: &Value, model: &str) {
        self.responses += 1;
        // Only search actions incur the search fee; opening/finding pages does not.
        let searches = response["output"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["type"] == "web_search_call" && item["action"]["type"] == "search")
            .count() as u64;
        self.web_searches += searches;
        let data = &response["usage"];
        let (Some(input), Some(output)) = (
            data["input_tokens"].as_u64(),
            data["output_tokens"].as_u64(),
        ) else {
            self.partial = true;
            self.estimated_cost_usd = None;
            return;
        };
        let cached = data["input_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap_or(0);
        let writes = data["input_tokens_details"]["cache_write_tokens"]
            .as_u64()
            .unwrap_or(0);
        let reasoning = data["output_tokens_details"]["reasoning_tokens"]
            .as_u64()
            .unwrap_or(0);
        self.input_tokens += input;
        self.cached_input_tokens += cached;
        self.cache_write_tokens += writes;
        self.output_tokens += output;
        self.reasoning_tokens += reasoning;
        let rates = match model {
            "gpt-6.1-sol" => Some((2.0, 0.10, 2.50, 10.0)),
            "gpt-6-sol" => Some((2.0, 0.20, 2.50, 10.0)),
            "gpt-6-astra" => Some((10.0, 1.0, 12.50, 50.0)),
            "gpt-6-luna" => Some((0.10, 0.01, 0.125, 0.50)),
            _ => None,
        };
        let tier = match response["service_tier"].as_str().unwrap_or("default") {
            "default" => Some(1.0),
            "flex" => Some(0.5),
            "priority" | "fast" => Some(2.0),
            _ => None,
        };
        let Some(((input_rate, cache_rate, write_rate, output_rate), tier)) = rates.zip(tier)
        else {
            self.estimated_cost_usd = None;
            return;
        };
        let Some(ordinary) = input
            .checked_sub(cached)
            .and_then(|n| n.checked_sub(writes))
        else {
            self.partial = true;
            self.estimated_cost_usd = None;
            return;
        };
        let long_context = input > 272_000;
        let input_multiplier = if long_context { 2.0 } else { 1.0 };
        let output_multiplier = if long_context { 1.5 } else { 1.0 };
        let token_cost = ((ordinary as f64 * input_rate
            + cached as f64 * cache_rate
            + writes as f64 * write_rate)
            * input_multiplier
            + output as f64 * output_rate * output_multiplier)
            * tier
            / 1_000_000.0;
        // Reasoning tokens are included in output_tokens; never charge them twice.
        if let Some(total) = &mut self.estimated_cost_usd {
            *total += token_cost + searches as f64 * SEARCH_USD;
        }
    }

    pub fn summary(&self) -> String {
        let cost = self
            .estimated_cost_usd
            .map(|usd| format!("${usd:.6} USD"))
            .unwrap_or_else(|| "unavailable (missing usage or unsupported model/tier)".into());
        format!(
            "Usage: {} input ({} cached, {} cache writes), {} output ({} reasoning), {} total tokens; {} web searches; estimated cost: {}{}",
            self.input_tokens,
            self.cached_input_tokens,
            self.cache_write_tokens,
            self.output_tokens,
            self.reasoning_tokens,
            self.input_tokens + self.output_tokens,
            self.web_searches,
            cost,
            if self.partial {
                "; partial usage—unreported charges excluded"
            } else {
                ""
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn sums_responses_with_cache_writes_and_search_fees_without_double_charging_reasoning() {
        let mut usage = Usage::default();
        let response = json!({"usage":{"input_tokens":2000,"input_tokens_details":{"cached_tokens":400,"cache_write_tokens":600},"output_tokens":200,"output_tokens_details":{"reasoning_tokens":150}},"output":[
            {"type":"web_search_call","action":{"type":"search"}},
            {"type":"web_search_call","action":{"type":"search"}},
            {"type":"web_search_call","action":{"type":"open_page"}}
        ]});
        usage.observe(&response, "gpt-6.1-sol");
        usage.observe(&response, "gpt-6.1-sol");
        assert_eq!(usage.input_tokens, 4000);
        assert_eq!(usage.output_tokens, 400);
        assert_eq!(usage.reasoning_tokens, 300);
        assert_eq!(usage.web_searches, 4);
        assert!((usage.estimated_cost_usd.unwrap() - 0.05108).abs() < 1e-9);
        assert!(usage.summary().contains("4400 total tokens"));
    }
    #[test]
    fn long_context_rates_apply_per_response_not_to_the_aggregate() {
        let mut usage = Usage::default();
        usage.observe(
            &json!({"usage":{"input_tokens":272001,"output_tokens":100},"output":[]}),
            "gpt-6.1-sol",
        );
        assert!((usage.estimated_cost_usd.unwrap() - 1.089504).abs() < 1e-9);
        let mut short = Usage::default();
        for _ in 0..2 {
            short.observe(
                &json!({"usage":{"input_tokens":200000,"output_tokens":100},"output":[]}),
                "gpt-6.1-sol",
            );
        }
        assert!((short.estimated_cost_usd.unwrap() - 0.802).abs() < 1e-9);
    }
    #[test]
    fn missing_usage_unknown_prices_and_invalid_counts_do_not_claim_zero_cost() {
        let mut usage = Usage::default();
        usage.observe(&json!({}), "gpt-6.1-sol");
        assert!(usage.partial && usage.estimated_cost_usd.is_none());
        let mut usage = Usage::default();
        usage.observe(
            &json!({"usage":{"input_tokens":20,"output_tokens":10}}),
            "unknown",
        );
        assert_eq!(usage.input_tokens, 20);
        assert!(usage.estimated_cost_usd.is_none());
        let mut usage = Usage::default();
        usage.observe(&json!({"usage":{"input_tokens":20,"output_tokens":10,"input_tokens_details":{"cached_tokens":30}}}), "gpt-6.1-sol");
        assert!(usage.partial && usage.estimated_cost_usd.is_none());
    }
}
