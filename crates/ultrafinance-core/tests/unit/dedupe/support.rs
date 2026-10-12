fn names(m: &Merchant) -> BTreeSet<String> {
    std::iter::once(&m.name)
        .chain(&m.aliases)
        .map(|s| crate::store::normalize(s))
        .filter(|s| s.len() >= 3)
        .collect()
}

fn pairs(snapshot: &Snapshot) -> BTreeSet<(usize, usize)> {
    focused_pairs(snapshot, None)
}

fn focused_pairs(
    snapshot: &Snapshot,
    focus: Option<&BTreeSet<String>>,
) -> BTreeSet<(usize, usize)> {
    let relevant = |a: usize, b: usize| {
        focus.is_none_or(|ids| {
            ids.contains(&snapshot.merchants[a].id) || ids.contains(&snapshot.merchants[b].id)
        })
    };
    let mut blocks: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, m) in snapshot.merchants.iter().enumerate() {
        for name in names(m) {
            blocks.entry(format!("name:{name}")).or_default().push(i);
        }
        if let Some(host) = host(&m.website) {
            blocks.entry(format!("host:{host}")).or_default().push(i);
        }
    }
    let normalized: Vec<_> = snapshot
        .merchants
        .iter()
        .map(|m| crate::store::normalize(&m.name))
        .collect();
    let mut fuzzy_blocks: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, name) in normalized.iter().enumerate() {
        let chars: Vec<_> = name.chars().collect();
        for trigram in chars.windows(3) {
            fuzzy_blocks
                .entry(trigram.iter().collect())
                .or_default()
                .push(i);
        }
    }
    let mut progress = crate::import_progress::Progress::dedupe(
        "candidate blocks",
        fuzzy_blocks.len() + blocks.len(),
    );
    let mut completed = 0;
    let mut fuzzy_checked = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    for block in fuzzy_blocks.values_mut() {
        block.sort_unstable();
        block.dedup();
        for (at, &a) in block.iter().enumerate() {
            for &b in &block[at + 1..] {
                if relevant(a, b)
                    && fuzzy_checked.insert((a, b))
                    && rapidfuzz::distance::levenshtein::normalized_similarity(
                        normalized[a].chars(),
                        normalized[b].chars(),
                    ) >= 0.85
                {
                    pairs.insert((a, b));
                }
            }
        }
        completed += 1;
        progress.advance(completed);
    }
    for block in blocks.values() {
        for (at, &left) in block.iter().enumerate() {
            for &right in &block[at + 1..] {
                if left != right && relevant(left, right) {
                    pairs.insert((left.min(right), left.max(right)));
                }
            }
        }
        completed += 1;
        progress.advance(completed);
    }
    progress.finish();
    pairs
}

fn question(snapshot: &Snapshot, left: usize, right: usize) -> Value {
    question_from_evidence(&evidence(snapshot, left), &evidence(snapshot, right))
}

fn stage(
    expected: &Snapshot,
    records: &[SourceRecord],
) -> Result<(Snapshot, Vec<String>, BTreeSet<String>)> {
    let mut staged = expected.clone();
    let mut source_index: BTreeMap<_, _> = staged
        .sources
        .iter()
        .enumerate()
        .map(|(i, (_, r))| ((r.source.clone(), r.external_id.clone()), i))
        .collect();
    let mut keys = BTreeSet::new();
    let mut ids = Vec::new();
    let mut touched = BTreeSet::new();
    let mut progress =
        crate::import_progress::Progress::new("staging source records", records.len());
    for (index, record) in records.iter().enumerate() {
        crate::store::validate(&record.merchant).with_context(|| {
            format!(
                "invalid merchant in source record {}/{} (entry {})",
                record.source,
                record.external_id,
                index + 1
            )
        })?;
        if record.source.trim().is_empty()
            || record.external_id.trim().is_empty()
            || !keys.insert((&record.source, &record.external_id))
        {
            bail!("source records must have unique nonblank source/external ID pairs");
        }
        let key = (record.source.clone(), record.external_id.clone());
        let previous = source_index.get(&key).copied();
        let id = previous
            .map(|i| staged.sources[i].0.clone())
            .unwrap_or_else(|| format!("mer_{}", uuid::Uuid::new_v4().simple()));
        if let Some(i) = previous {
            staged.sources[i] = (id.clone(), record.clone());
        } else {
            source_index.insert(key, staged.sources.len());
            staged.sources.push((id.clone(), record.clone()));
        }
        touched.insert(id.clone());
        ids.push(id);
        progress.advance(index + 1);
    }
    progress.finish();
    staged
        .sources
        .sort_by(|a, b| (&a.1.source, &a.1.external_id).cmp(&(&b.1.source, &b.1.external_id)));
    let mut progress =
        crate::import_progress::Progress::new("preparing merchant identities", touched.len());
    let manual: BTreeMap<_, _> = staged.manual.iter().map(|m| (&m.id, m)).collect();
    let mut evidence: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for (id, record) in &staged.sources {
        evidence.entry(id).or_default().push(record);
    }
    let mut merchant_index: BTreeMap<_, _> = staged
        .merchants
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.clone(), i))
        .collect();
    for (index, id) in touched.iter().enumerate() {
        let mut merchant = manual.get(id).map(|m| (*m).clone());
        for record in evidence
            .get(id)
            .context("incoming identity has no source evidence")?
        {
            let m = merchant.get_or_insert_with(|| record.merchant.clone());
            combine(m, &record.merchant);
            if !record.url.is_empty() {
                m.sources.push(record.url.clone());
            }
        }
        let mut merchant = merchant.context("incoming identity has no evidence")?;
        merchant.id = id.clone();
        merchant.aliases.sort();
        merchant.aliases.dedup();
        merchant.sources.sort();
        merchant.sources.dedup();
        if let Some(&i) = merchant_index.get(id) {
            staged.merchants[i] = merchant;
        } else {
            merchant_index.insert(id.clone(), staged.merchants.len());
            staged.merchants.push(merchant);
        }
        progress.advance(index + 1);
    }
    progress.finish();
    staged.merchants.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((staged, ids, touched))
}

fn deterministic_plan(
    expected: &Snapshot,
    staged: &Snapshot,
    touched: &BTreeSet<String>,
    dry_run: bool,
) -> Report {
    let mut buckets: BTreeMap<_, Vec<&Merchant>> = BTreeMap::new();
    let mut progress = crate::import_progress::Progress::new(
        "indexing deterministic identities",
        staged.merchants.len(),
    );
    for (index, merchant) in staged.merchants.iter().enumerate() {
        if let Some(key) = deterministic_key(merchant) {
            buckets.entry(key).or_default().push(merchant);
        }
        progress.advance(index + 1);
    }
    progress.finish();
    let established: BTreeSet<_> = expected.merchants.iter().map(|m| &m.id).collect();
    let manual: BTreeSet<_> = expected.manual.iter().map(|m| &m.id).collect();
    let mut source_counts: BTreeMap<_, usize> = BTreeMap::new();
    for (id, _) in &expected.sources {
        *source_counts.entry(id).or_default() += 1;
    }
    let mut report = Report {
        catalog_revision: expected.revision,
        dry_run,
        model: "deterministic".into(),
        threshold: 1.0,
        candidates: 0,
        errors: 0,
        merchants_scanned: staged.merchants.len(),
        merchants: vec![],
        groups: vec![],
        decisions: vec![],
        run_id: None,
        details_truncated: false,
    };
    for (_, mut members) in buckets {
        if members.len() < 2 || !members.iter().any(|m| touched.contains(&m.id)) {
            continue;
        }
        // Multiple reviewed identities are a conflict, not a tie to break.
        if members.iter().filter(|m| manual.contains(&m.id)).count() > 1 {
            continue;
        }
        members.sort_by_key(|m| {
            (
                !manual.contains(&m.id),
                !established.contains(&m.id),
                std::cmp::Reverse(source_counts.get(&m.id).copied().unwrap_or(0)),
                &m.id,
            )
        });
        for other in &members[1..] {
            report.decisions.push(Decision {
                left: members[0].id.clone(),
                right: other.id.clone(),
                answer: rule_answer(members[0]),
                accepted: true,
                method: "rule".into(),
                error: None,
            });
        }
        report
            .groups
            .push(members.iter().map(|m| m.id.clone()).collect());
        report.merchants.extend(members.into_iter().cloned());
    }
    report.candidates = report.decisions.len();
    eprintln!(
        "Import: deterministic reconciliation — {} merge groups, {} duplicate identities; no Jev calls",
        report.groups.len(),
        report.candidates
    );
    report
}
