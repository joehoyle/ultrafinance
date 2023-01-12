pub fn display_option<T: std::fmt::Display>(value: &Option<T>) -> impl std::fmt::Display {
    match value {
        Some(v) => format!("{}", v),
        None => "".to_owned(),
    }
}
