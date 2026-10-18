impl Trace {
    pub fn with_writer(writer: impl Write + Send + 'static, credential: &str) -> Self {
        Self {
            writer: Some(Box::new(writer)),
            credential: credential.into(),
        }
    }
}
