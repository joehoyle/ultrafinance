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
