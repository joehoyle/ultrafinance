use serde_json::json;

fn parse_choice(value: &Value, candidates: &[Merchant], threshold: f64) -> Result<EnrichResponse> {
    parse_choice_answer(&value["answers"]["merchant"], candidates, threshold)
}
