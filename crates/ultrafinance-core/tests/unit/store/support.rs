impl MerchantStore {
    pub(crate) fn apply_reconciled_import(
        &self,
        expected: &crate::dedupe::Snapshot,
        staged: &crate::dedupe::Snapshot,
        records: &[SourceRecord],
        identities: &[String],
        groups: &[Vec<String>],
    ) -> Result<(ImportDelta, Option<String>)> {
        self.0
            .apply_reconciled_import(expected, staged, records, identities, groups)
    }
}

impl MerchantStore {
    pub fn dedupe_snapshot(&self) -> Result<crate::dedupe::Snapshot> {
        self.0.dedupe_snapshot()
    }
}
