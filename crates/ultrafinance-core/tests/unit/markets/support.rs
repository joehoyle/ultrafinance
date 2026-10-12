pub(crate) fn source_region(record: &SourceRecord) -> Option<String> {
    let region = dataset_region(record)?.to_ascii_uppercase();
    valid_country(&region).then_some(region)
}
