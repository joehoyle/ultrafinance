use anyhow::Result;
use serde_json::Value;
use std::io::{self, Write};

pub(super) struct Trace {
    writer: Option<Box<dyn Write + Send>>,
    credential: String,
}
impl Trace {
    pub fn new(enabled: bool, credential: &str) -> Self {
        Self {
            writer: enabled.then(|| Box::new(io::stderr()) as Box<dyn Write + Send>),
            credential: credential.into(),
        }
    }
    pub fn event(&mut self, step: usize, label: &str, value: &Value) -> Result<()> {
        if let Some(writer) = &mut self.writer {
            let json = serde_json::to_string_pretty(value)?;
            // Only JSON bodies are traced, never HTTP headers or environment.
            // Also redact echoes of the provider credential in body contents.
            let json = if self.credential.is_empty() {
                json
            } else {
                json.replace(&self.credential, "[REDACTED]")
            };
            writeln!(writer, "\n[research step {step}: {label}]\n{json}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/research/trace/tests.rs"]
mod tests;

#[cfg(test)]
include!("../../tests/unit/research/trace/support.rs");
