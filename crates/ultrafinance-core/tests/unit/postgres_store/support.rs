impl PostgresStore {
    pub fn dedupe_snapshot(&self) -> Result<crate::dedupe::Snapshot> {
        self.run(|client| {
            let mut tx = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()?;
            let snapshot = postgres_dedupe_snapshot(&mut tx)?;
            tx.commit()?;
            Ok(snapshot)
        })
    }
}

impl PostgresStore {
    pub fn apply_reconciled_import(
        &self,
        expected: &crate::dedupe::Snapshot,
        staged: &crate::dedupe::Snapshot,
        records: &[SourceRecord],
        identities: &[String],
        groups: &[Vec<String>],
    ) -> Result<(super::ImportDelta, Option<String>)> {
        let (expected, staged, records, identities, groups) = (
            expected.clone(),
            staged.clone(),
            records.to_vec(),
            identities.to_vec(),
            groups.to_vec(),
        );
        if records.len() != identities.len() {
            bail!("invalid import identity plan");
        }
        self.run(move |client| {
        crate::dedupe::validate_plan(&staged, &staged, &groups)?;
        let mut tx = import_transaction(client)?;
        eprintln!("Import: waiting for database write lock");
        write_lock(&mut tx)?;
        eprintln!("Import: validating catalog snapshot");
        tx.batch_execute(crate::dedupe::SCHEMA).context("cannot initialize dedupe tables; run database init with a schema-owner connection")?;
        tx.batch_execute("LOCK TABLE merchants,manual_merchants,source_records,location_records,descriptor_resolutions,merchant_redirects IN SHARE ROW EXCLUSIVE MODE")?;
        let current = postgres_dedupe_snapshot(&mut tx)?;
        crate::dedupe::validate_plan(&expected, &current, &[])?;
        // New duplicate source records go directly to their survivor. Only
        // already-persisted identities need retirement and redirects.
        let targets: HashMap<_,_> = groups.iter().flat_map(|g| g.iter().map(|id| (id, &g[0]))).collect();
        let identities: Vec<_> = identities.iter().map(|id| targets.get(id).copied().unwrap_or(id).clone()).collect();
        let existing: HashSet<_> = expected.merchants.iter().map(|m| &m.id).collect();
        let persisted_groups: Vec<Vec<String>> = groups.iter().filter_map(|g| {
            let mut group=vec![g[0].clone()];
            group.extend(g[1..].iter().filter(|id| existing.contains(id)).cloned());
            (group.len()>1).then_some(group)
        }).collect();
        let delta = import_records(&mut tx, &records, Some(&identities), Some(&expected))?;
        let run_id = if groups.is_empty() { None } else {
            let run_id = uuid::Uuid::new_v4().to_string();
            merge_groups(&mut tx, &persisted_groups)?;
            Some(run_id)
        };
        eprintln!("Import: committing transaction");
        tx.commit()?;
        eprintln!("Import: committed — {} added, {} updated, {} unchanged", delta.added, delta.updated, delta.unchanged);
        Ok((delta, run_id))
    })
    }
}

fn postgres_dedupe_snapshot(c: &mut impl GenericClient) -> Result<crate::dedupe::Snapshot> {
    let sources = c
        .query(
            "SELECT s.*,source_category_ids(s.source,s.external_id) AS input_category_ids,p.attribution,p.license,p.url FROM source_records s JOIN source_provenance p ON p.id=s.source_provenance_id ORDER BY s.source,external_id",
            &[],
        )?
        .iter()
        .map(|r| Ok((r.try_get("merchant_id")?, rows::source(r)?)))
        .collect::<Result<_>>()?;
    let redirects_exist: bool = c
        .query_one(
            "SELECT to_regclass('public.merchant_redirects') IS NOT NULL",
            &[],
        )?
        .get(0);
    let redirects = if redirects_exist {
        c.query(
            "SELECT retired_id,merchant_id FROM merchant_redirects ORDER BY retired_id",
            &[],
        )?
        .into_iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect()
    } else {
        vec![]
    };
    Ok(crate::dedupe::Snapshot {
        merchants: c
            .query("SELECT *,merchant_category_ids(id) AS category_ids FROM merchants ORDER BY id", &[])?
            .iter()
            .map(rows::merchant)
            .collect::<Result<_>>()?,
        manual: c
            .query("SELECT *,merchant_category_ids(id,true) AS category_ids FROM manual_merchants ORDER BY id", &[])?
            .iter()
            .map(rows::merchant)
            .collect::<Result<_>>()?,
        sources,
        locations: c
            .query(&format!("{} ORDER BY l.id", rows::LOCATIONS), &[])?
            .iter()
            .map(rows::location)
            .collect::<Result<_>>()?,
        redirects,
        location_redirects: location_redirects(c)?.into_iter().collect(),
        revision: Some(
            c.query_one("SELECT revision FROM catalog_revision", &[])?
                .get(0),
        ),
    })
}
