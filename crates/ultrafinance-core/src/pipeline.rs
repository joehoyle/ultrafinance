use crate::{EnrichRequest, Enricher, store::Candidate};
use anyhow::Result;

impl Enricher {
    pub(crate) async fn retrieve(&self, request: &EnrichRequest) -> Result<Vec<Candidate>> {
        let store = self.store.clone();
        let input = request.clone();
        tokio::task::spawn_blocking(move || store.search_request(&input, 10)).await?
    }
}
