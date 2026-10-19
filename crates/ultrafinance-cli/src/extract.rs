//! Standalone extraction experiment; extracted clues never enter trusted catalog data.
use anyhow::{Context, Result, bail};
use clap::Args as ClapArgs;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use ultrafinance_core::EnrichRequest;

const MODEL: &str = "@cf/cloudflare/clairic";
const MAX_RESPONSE: usize = 64 * 1024;
const FIELDS: [(&str, &str); 10] = [
    (
        "processor",
        "Payment processor or payment platform explicitly present in the descriptor. Copy its exact text; do not expand abbreviations or confuse it with the merchant. Null if absent.",
    ),
    (
        "merchant",
        "Customer-facing merchant name as written, excluding a processor wrapper, reference numbers, and a clearly separate location. Preserve numbers belonging to the business name. Do not invent or expand a business name. Null if no merchant name is identifiable.",
    ),
    (
        "address",
        "Street address explicitly written in the descriptor, including a street or building number when present. Exclude separately written city, region, postal code, and country. Do not treat a store number or payment reference as a street address. Copy exact text. Null if absent or ambiguous.",
    ),
    (
        "city",
        "City, town, village, or other locality explicitly written in the descriptor. Exclude separately written state/province/region, postal code, and country. Do not infer from a merchant name, address, postal code, website, phone code, or processor. Copy exact text. Null if absent or ambiguous.",
    ),
    (
        "region",
        "Province, state, or region explicitly written in the descriptor, preserving abbreviations. Null if absent or ambiguous.",
    ),
    (
        "country",
        "Country name or country code explicitly written in the descriptor. Do not infer from other geography or a merchant. Null if absent or ambiguous.",
    ),
    (
        "postal_code",
        "Postal or ZIP code explicitly written in the descriptor, preserving letters, spaces, hyphens, and leading zeros. Do not infer it from an address or locality, or treat an unlabeled store number or transaction reference as a postal code. Copy exact text. Null if absent or ambiguous.",
    ),
    (
        "store_number",
        "Explicit store or branch identifier, preserving leading zeros. Do not treat an unlabeled terminal number or transaction reference as a store number. Null if absent or ambiguous.",
    ),
    (
        "reference",
        "Explicit transaction, order, or payment reference identifier, preserving leading zeros. Do not confuse with a store number or a number in the business name. Null if absent or ambiguous.",
    ),
    (
        "website",
        "Website or domain explicitly present in the descriptor. Copy the original text without adding a scheme or completing a domain. Null if absent.",
    ),
];

#[derive(ClapArgs)]
pub struct Args {
    pub description: String,
    /// Show the exact provider payload and local interpretation without a network call.
    #[arg(long)]
    pub dry_run: bool,
    /// Print compact JSON (default: pretty JSON).
    #[arg(long)]
    pub json: bool,
}

fn payload(description: &str) -> Value {
    let fields: BTreeMap<_, _> = FIELDS
        .iter()
        .map(|(name, instructions)| (*name, json!({"type": "text", "instructions": instructions})))
        .collect();
    json!({"model": "clairic", "state": description, "fields": fields})
}

fn probability(answer: &Value, name: &str) -> Result<f64> {
    let value = answer[name]
        .as_f64()
        .context("missing extraction probability")?;
    if !(0.0..=1.0).contains(&value) {
        bail!("invalid extraction probability");
    }
    Ok(value)
}

fn extracted_fields(result: &Value, description: &str) -> Result<Value> {
    if result["truncated"] != false {
        bail!("Clairic response is truncated or missing truncation status");
    }
    let answers = result["answers"]
        .as_object()
        .context("missing Clairic answers")?;
    let chars: Vec<_> = description.chars().collect();
    let mut fields = serde_json::Map::new();
    for (name, _) in FIELDS {
        let answer = answers
            .get(name)
            .with_context(|| format!("missing Clairic field {name}"))?;
        if answer["type"] != "text" {
            bail!("invalid Clairic field type for {name}");
        }
        let value = answer.get("value").context("missing extraction value")?;
        let confidence = probability(answer, "confidence")?;
        let p_null = probability(answer, "p_null")?;
        let span = if value.is_null() {
            // Provider guesses and their spans are not extracted evidence.
            Value::Null
        } else {
            let text = value.as_str().context("invalid extraction text")?;
            let span = answer["span"]
                .as_array()
                .context("missing extraction span")?;
            let offsets: Option<Vec<usize>> = span
                .iter()
                .map(|n| n.as_u64().and_then(|n| usize::try_from(n).ok()))
                .collect();
            let offsets = offsets.context("invalid extraction offsets")?;
            if offsets.len() != 2
                || offsets[0] >= offsets[1]
                || offsets[1] > chars.len()
                || chars[offsets[0]..offsets[1]].iter().collect::<String>() != text
            {
                bail!("Clairic field {name} does not match its source span");
            }
            json!(offsets)
        };
        fields.insert(
            name.into(),
            json!({"value": value, "span": span,
            "confidence": confidence, "p_null": p_null}),
        );
    }
    Ok(Value::Object(fields))
}

async fn infer(account: &str, token: &str, body: &Value) -> Result<Value> {
    if account.len() != 32 || !account.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("CLOUDFLARE_ACCOUNT_ID must be a 32-character hexadecimal account ID");
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let mut response = client
        .post(format!(
            "https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/{MODEL}"
        ))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .context("Clairic request failed")?;
    if !response.status().is_success() {
        // Do not echo provider errors which may contain request data or credentials.
        bail!("Clairic returned HTTP {}", response.status());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("cannot read Clairic response")?
    {
        if bytes.len() + chunk.len() > MAX_RESPONSE {
            bail!("Clairic response exceeds 64 KiB");
        }
        bytes.extend_from_slice(&chunk);
    }
    let envelope: Value = serde_json::from_slice(&bytes).context("invalid Clairic JSON")?;
    if envelope["success"] != true {
        bail!("Cloudflare reported an unsuccessful extraction");
    }
    envelope
        .get("result")
        .cloned()
        .context("missing Cloudflare result")
}

pub async fn run(args: Args) -> Result<()> {
    let request: EnrichRequest = serde_json::from_value(json!({"description": args.description}))?;
    request.validate()?;
    let body = payload(&request.description);
    let local = ultrafinance_core::interpretation::interpret(&request);
    let report = if args.dry_run {
        json!({"dry_run": true, "request": body, "local_interpretation": local})
    } else {
        let account = std::env::var("CLOUDFLARE_ACCOUNT_ID")
            .context("extract requires CLOUDFLARE_ACCOUNT_ID")?;
        let token = std::env::var("CLOUDFLARE_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .context("extract requires CLOUDFLARE_AUTH_TOKEN with Workers AI access")?;
        let started = Instant::now();
        let result = infer(&account, &token, &body).await?;
        let fields = extracted_fields(&result, &request.description)?;
        let location: BTreeMap<_, _> = ["address", "city", "region", "postal_code", "country"]
            .into_iter()
            .map(|name| (name, fields[name]["value"].clone()))
            .collect();
        json!({"description": request.description, "model": MODEL, "status": "unverified",
            "fields": fields, "location": location,
            "elapsed_ms": started.elapsed().as_millis(), "usage": result["usage"],
            "local_interpretation": local})
    };
    crate::output::write_output(&if args.json {
        serde_json::to_string(&report)?
    } else {
        serde_json::to_string_pretty(&report)?
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response() -> Value {
        let answers: BTreeMap<_, _> = FIELDS
            .iter()
            .map(|(name, _)| {
                (
                    *name,
                    json!({"type": "text", "value": null, "guess": "wrong", "span": [99, 100],
                "confidence": 0.99, "p_null": 1.0}),
                )
            })
            .collect();
        json!({"truncated": false, "answers": answers})
    }

    #[test]
    fn missing_values_never_promote_guesses_or_spans() {
        let fields = extracted_fields(&response(), "SQ *CAFÉ").unwrap();
        assert!(fields["merchant"]["value"].is_null());
        assert!(fields["merchant"]["span"].is_null());
        assert!(fields["merchant"].get("guess").is_none());
    }

    #[test]
    fn validates_unicode_character_spans_and_rejects_invented_text() {
        let mut result = response();
        result["answers"]["merchant"] = json!({"type": "text", "value": "CAFÉ",
            "span": [4, 8], "confidence": 0.9, "p_null": 0.1});
        assert_eq!(
            extracted_fields(&result, "SQ *CAFÉ").unwrap()["merchant"]["value"],
            "CAFÉ"
        );
        result["answers"]["merchant"]["value"] = json!("Cafe");
        assert!(extracted_fields(&result, "SQ *CAFÉ").is_err());
    }

    #[test]
    fn rejects_partial_truncated_and_malformed_answers() {
        let mut result = response();
        result["truncated"] = json!(true);
        assert!(extracted_fields(&result, "SHOP").is_err());
        result["truncated"] = json!(false);
        result["answers"]["country"]["confidence"] = json!(1.5);
        assert!(extracted_fields(&result, "SHOP").is_err());
        result["answers"].as_object_mut().unwrap().remove("country");
        assert!(extracted_fields(&result, "SHOP").is_err());
    }
}
