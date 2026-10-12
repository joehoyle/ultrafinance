impl Enricher {
    pub(crate) async fn enrich_batch_candidates(
        &self,
        inputs: Vec<(EnrichRequest, Result<Vec<store::Candidate>>)>,
    ) -> Vec<Result<EnrichResponse>> {
        self.enrich_batch_candidates_traced(inputs).await.0
    }
}
