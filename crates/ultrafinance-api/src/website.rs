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

// Keep the useful fields first and scalar arrays on one line. Only whitespace
// and object key order change; the saved response remains the source of truth.
fn display_json(value: &Value, depth: usize) -> String {
    match value {
        Value::Object(object) if !object.is_empty() => {
            let priority = [
                "description",
                "amount",
                "currency",
                "country",
                "merchant",
                "status",
                "data",
                "id",
                "name",
                "markets",
                "website",
                "logo_url",
                "logo_source",
                "location",
                "attributions",
            ];
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_by_key(|key| {
                priority
                    .iter()
                    .position(|name| name == key)
                    .unwrap_or(priority.len())
            });
            let fields: Vec<_> = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}: {}",
                        serde_json::to_string(key).unwrap(),
                        display_json(&object[key], depth + 1)
                    )
                })
                .collect();
            let inline = format!("{{{}}}", fields.join(", "));
            if depth > 0 && inline.len() <= 64 && !inline.contains('\n') {
                return inline;
            }
            format!(
                "{{\n{}\n{}}}",
                fields
                    .iter()
                    .map(|field| format!("{}{field}", "  ".repeat(depth + 1)))
                    .collect::<Vec<_>>()
                    .join(",\n"),
                "  ".repeat(depth)
            )
        }
        Value::Array(values) if !values.is_empty() => {
            let fields: Vec<_> = values
                .iter()
                .map(|value| display_json(value, depth + 1))
                .collect();
            let inline = format!("[{}]", fields.join(", "));
            if inline.len() <= 64 && !inline.contains('\n') {
                return inline;
            }
            format!(
                "[\n{}\n{}]",
                fields
                    .iter()
                    .map(|field| format!("{}{field}", "  ".repeat(depth + 1)))
                    .collect::<Vec<_>>()
                    .join(",\n"),
                "  ".repeat(depth)
            )
        }
        _ => serde_json::to_string(value).unwrap(),
    }
}

fn highlight_json(value: &str) -> String {
    let mut result = String::new();
    let mut offset = 0;
    while let Some(start) = value[offset..].find('"').map(|index| offset + index) {
        result.push_str(&escape(&value[offset..start]));
        let mut end = start + 1;
        while end < value.len() {
            match value.as_bytes()[end] {
                b'\\' => end += 2,
                b'"' => {
                    end += 1;
                    break;
                }
                _ => end += 1,
            }
        }
        let class = if value[end..].trim_start().starts_with(':') {
            "key"
        } else {
            "string"
        };
        result.push_str(&format!(
            "<span class=\"{class}\">{}</span>",
            escape(&value[start..end])
        ));
        offset = end;
    }
    result.push_str(&escape(&value[offset..]));
    result
}

fn render(template: &str, example: &Example) -> String {
    let request = display_json(&example.request, 0);
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
    let response = display_json(&example.response, 0);
    let captured_label = format!(
        "{} UTC",
        example
            .captured_at
            .trim_end_matches('Z')
            .replace('T', " · ")
    );
    let replacements = [
        ("{{EXAMPLE_API_REQUEST}}", highlight_json(&api)),
        ("{{EXAMPLE_CLI_REQUEST}}", escape(&cli)),
        ("{{EXAMPLE_RESPONSE}}", highlight_json(&response)),
        ("{{EXAMPLE_CAPTURED_AT}}", escape(&example.captured_at)),
        ("{{EXAMPLE_CAPTURED_LABEL}}", escape(&captured_label)),
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
