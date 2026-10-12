#[test]
fn mappings_are_scoped_verified_explicitly_and_revocable() -> anyhow::Result<()> {
    super::exercise(&crate::store::MerchantStore::temporary()?)
}
