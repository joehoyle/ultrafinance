use crate::{EnrichRequest, Enricher, store::Candidate};
use anyhow::Result;

/// Read-only enrichment evidence for an explicit research caller. No provider,
/// discovery, audit logging or descriptor learning runs here.
pub struct LocalEnrichment {
    pub response: crate::EnrichResponse,
    pub candidates: Vec<Candidate>,
    pub interpretation: crate::interpretation::Interpretation,
}

/// Run the enrichment fast path and geography extraction. Ambiguous catalog
/// candidates remain unresolved and are returned for the caller to investigate.
pub async fn enrich_locally(
    request: &EnrichRequest,
    store: crate::store::MerchantStore,
) -> Result<LocalEnrichment> {
    request.validate()?;
    let enricher = Enricher {
        persist_matches: false,
        discovery: None,
        client: reqwest::Client::new(),
        provider_url: String::new(),
        api_key: None,
        model: String::new(),
        threshold: 0.95,
        store,
    };
    let candidates = enricher.retrieve(request).await?;
    let local_candidates = if crate::batch::exact_match(request, &candidates) {
        candidates.clone()
    } else {
        vec![]
    };
    let (mut results, _) = enricher
        .enrich_batch_candidates_traced(vec![(request.clone(), Ok(local_candidates))])
        .await;
    Ok(LocalEnrichment {
        response: results.remove(0)?,
        candidates,
        interpretation: crate::interpretation::interpret(request),
    })
}

impl Enricher {
    pub(crate) async fn retrieve(&self, request: &EnrichRequest) -> Result<Vec<Candidate>> {
        let store = self.store.clone();
        let input = request.clone();
        tokio::task::spawn_blocking(move || store.search_request(&input, 10)).await?
    }
}
