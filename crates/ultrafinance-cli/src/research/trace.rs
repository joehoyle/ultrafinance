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
    #[cfg(test)]
    pub fn with_writer(writer: impl Write + Send + 'static, credential: &str) -> Self {
        Self {
            writer: Some(Box::new(writer)),
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
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn trace_is_opt_in_and_redacts_credential_echoes_in_nested_body_strings() -> Result<()> {
        let mut disabled = Trace::new(false, "secret-test-key");
        disabled.event(1, "request", &json!({"description":"private"}))?;
        assert!(disabled.writer.is_none());
        let capture = Capture::default();
        let mut trace = Trace::with_writer(capture.clone(), "secret-test-key");
        trace.event(
            2,
            "response",
            &json!({"output":[{"arguments":"{\"echo\":\"secret-test-key\"}"}]}),
        )?;
        let text = String::from_utf8(capture.0.lock().unwrap().clone())?;
        assert!(text.contains("step 2: response"));
        assert!(text.contains("[REDACTED]"));
        assert!(!text.contains("secret-test-key"));
        Ok(())
    }
}
