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
