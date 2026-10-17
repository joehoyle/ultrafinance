use serde::Deserialize;
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Example {
    endpoint: String,
    captured_at: String,
    request: Value,
    response: Value,
}

static HOMEPAGE: LazyLock<String> = LazyLock::new(|| {
    let example: Example = serde_json::from_str(include_str!("../../../website/example.json"))
        .expect("embedded homepage example must be valid JSON");
    render(include_str!("../../../website/index.html"), &example)
});

pub fn homepage() -> &'static str {
    &HOMEPAGE
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn render(template: &str, example: &Example) -> String {
    let request = serde_json::to_string_pretty(&example.request).unwrap();
    let api = format!(
        "curl {} \\\n  -H 'Content-Type: application/json' \\\n  -d {}",
        shell_quote(&example.endpoint),
        shell_quote(&request),
    );
    let mut cli = format!(
        "cargo run -- enrich {}",
        shell_quote(example.request["description"].as_str().unwrap())
    );
    for field in ["amount", "currency", "country", "date"] {
        if let Some(value) = example.request[field].as_str() {
            cli.push_str(&format!(" \\\n  --{field} {}", shell_quote(value)));
        }
    }
    let response = serde_json::to_string_pretty(&example.response).unwrap();
    let replacements = [
        ("{{EXAMPLE_API_REQUEST}}", escape(&api)),
        ("{{EXAMPLE_CLI_REQUEST}}", escape(&cli)),
        ("{{EXAMPLE_RESPONSE}}", escape(&response)),
        ("{{EXAMPLE_CAPTURED_AT}}", escape(&example.captured_at)),
    ];
    // Replace only template tokens, never tokens contained in API data.
    let mut html = String::new();
    let mut remaining = template;
    while let Some(start) = remaining.find("{{EXAMPLE_") {
        html.push_str(&remaining[..start]);
        let (token, value) = replacements
            .iter()
            .find(|(token, _)| remaining[start..].starts_with(token))
            .expect("unknown homepage example placeholder");
        html.push_str(value);
        remaining = &remaining[start + token.len()..];
    }
    html.push_str(remaining);
    html
}

#[cfg(test)]
#[path = "../tests/unit/website/tests.rs"]
mod tests;
