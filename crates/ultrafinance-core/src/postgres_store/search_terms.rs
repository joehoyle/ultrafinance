//! Keep each bounded term visible to PostgreSQL's selectivity estimator.
//! A parameterized LATERAL scan over unnest hides the actual term, causing
//! LIMIT to favor repeated full scans when rare terms have few or no matches.
use super::*;
use postgres::{Row, types::ToSql};

fn statement(count: usize, limit: usize, plain: bool, typed: bool) -> String {
    let function = if plain {
        "plainto_tsquery"
    } else {
        "to_tsquery"
    };
    let filter = if typed {
        format!(
            " AND merchant_id IN (SELECT merchant_key FROM merchants WHERE counterparty_type=${}::text)",
            count + 1
        )
    } else {
        String::new()
    };
    let branches = (1..=count)
        .map(|index| format!(
            "(SELECT merchant_id FROM merchant_search WHERE tokens @@ {function}('simple',${index}::text){filter} LIMIT {limit})"
        ))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    format!(
        "SELECT m.id,m.name,m.aliases,m.markets FROM (SELECT DISTINCT pool.merchant_id FROM ({branches}) pool) s JOIN merchants m ON m.merchant_key=s.merchant_id"
    )
}

pub(super) fn query(
    tx: &mut Transaction<'_>,
    terms: &[String],
    limit: usize,
    plain: bool,
    entity_type: Option<&str>,
) -> Result<Vec<Row>> {
    if terms.is_empty() {
        return Ok(vec![]);
    }
    // Only placeholder numbers, the fixed function name, and the internal
    // per-term limit enter SQL. All descriptor text remains bound parameters.
    let mut params: Vec<&(dyn ToSql + Sync)> = terms
        .iter()
        .map(|term| term as &(dyn ToSql + Sync))
        .collect();
    if let Some(ref entity_type) = entity_type {
        params.push(entity_type);
        return Ok(tx.query(&statement(terms.len(), limit, plain, true), &params)?);
    }
    Ok(tx.query(&statement(terms.len(), limit, plain, false), &params)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn term_pools_keep_rare_matches_limits_and_parameter_safety() -> Result<()> {
        let base = std::env::var("ULTRAFINANCE_TEST_DATABASE_URL")
            .unwrap_or_else(|_| crate::store::LOCAL_DATABASE_URL.into());
        let store = PostgresStore::temporary(&base)?;
        store.run(|client| {
            let mut tx = client.transaction()?;
            tx.batch_execute("CREATE TEMP TABLE merchants (merchant_key bigint, id text, name text, aliases text[], markets text[]);
                CREATE TEMP TABLE merchant_search (merchant_id bigint, tokens tsvector);
                INSERT INTO merchants SELECT n,n::text,'Common '||n,ARRAY[]::text[],ARRAY['CA'] FROM generate_series(1,100) n;
                INSERT INTO merchant_search SELECT n,to_tsvector('simple',CASE WHEN n=100 THEN 'common rare' ELSE 'common' END) FROM generate_series(1,100) n;")?;
            let terms = vec!["'common'".into(), "'common' & 'rare'".into(), "'missing'".into()];
            let rows = query(&mut tx, &terms, 1, false, None)?;
            let ids: HashSet<String> = rows.iter().map(|row| row.get(0)).collect();
            assert!(ids.contains("100"));
            assert!(ids.len() <= 2);
            let rows = query(&mut tx, &["rare".into(), "rare".into()], 32, true, None)?;
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].get::<_, String>(0), "100");
            assert!(query(&mut tx, &["'; DROP TABLE merchants; --".into()], 32, true, None)?.is_empty());
            assert_eq!(tx.query_one("SELECT count(*) FROM merchants", &[])?.get::<_,i64>(0), 100);
            assert!(query(&mut tx, &[], 32, false, None)?.is_empty());
            Ok(())
        })
    }
}
