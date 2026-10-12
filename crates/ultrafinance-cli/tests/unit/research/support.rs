async fn investigate(
    client: &reqwest::Client,
    endpoint: &str,
    key: &str,
    model: &str,
    request: &EnrichRequest,
    store: &MerchantStore,
) -> Result<Report> {
    investigate_with_usage(
        client,
        endpoint,
        key,
        model,
        request,
        store,
        &mut ResearchSession {
            usage: Usage::default(),
            trace: Trace::new(false, key),
        },
    )
    .await
}
