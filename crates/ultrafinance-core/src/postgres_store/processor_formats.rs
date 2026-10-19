use super::*;
use crate::processor_formats::ProcessorFormat;

pub(super) fn read(client: &mut impl GenericClient) -> Result<Vec<ProcessorFormat>> {
    client.query("SELECT id,processor_id,pattern,sources,reviewed_on::text,scope,notes,enabled FROM processor_formats ORDER BY id", &[])?
        .into_iter().map(|row| Ok(ProcessorFormat {
            id: row.get(0), processor_id: row.get(1), pattern: row.get(2), sources: row.get(3),
            reviewed_on: row.get(4), scope: row.get(5), notes: row.get(6), enabled: row.get(7),
        })).collect()
}

impl PostgresStore {
    pub fn processor_formats(&self) -> Result<Vec<ProcessorFormat>> {
        self.run(read)
    }

    pub fn processor_format_matches(
        &self,
        descriptions: Vec<String>,
    ) -> Result<Vec<Vec<crate::processor_formats::FormatMatch>>> {
        let owner = self.0.clone();
        self.run(move |client| {
            let mut tx = client
                .build_transaction()
                .isolation_level(IsolationLevel::RepeatableRead)
                .read_only(true)
                .start()?;
            let formats = owner.reads.lock().unwrap().processor_formats(&mut tx)?;
            let mut matches = vec![Vec::new(); descriptions.len()];
            for cached in formats.iter() {
                for (index, description) in descriptions.iter().enumerate() {
                    if let Some(capture) =
                        crate::processor_formats::capture(&cached.regex, description)
                    {
                        matches[index].push(crate::processor_formats::FormatMatch {
                            format: cached.format.clone(),
                            processor: cached.processor.clone(),
                            capture,
                        });
                    }
                }
            }
            tx.commit()?;
            Ok(matches)
        })
    }

    pub fn save_processor_formats(
        &self,
        formats: Vec<ProcessorFormat>,
        dry_run: bool,
    ) -> Result<()> {
        self.run(move |client| {
            let mut tx = client.transaction()?;
            write_lock(&mut tx)?;
            let mut combined = read(&mut tx)?;
            for format in &formats {
                let kind: Option<String> = tx.query_opt("SELECT counterparty_type FROM merchants WHERE id=$1 FOR KEY SHARE", &[&format.processor_id])?.map(|row| row.get(0));
                if kind.as_deref() != Some(crate::CounterpartyType::PaymentProcessor.as_str()) {
                    bail!("processor format target must be an existing payment_processor");
                }
                combined.retain(|existing| existing.id != format.id);
                combined.push(format.clone());
            }
            crate::processor_formats::validate_set(&combined)?;
            for format in formats {
                tx.execute("INSERT INTO processor_formats(id,processor_id,pattern,sources,reviewed_on,scope,notes,enabled)
                    VALUES($1,$2,$3,$4,$5::text::date,$6,$7,$8)
                    ON CONFLICT(id) DO UPDATE SET processor_id=EXCLUDED.processor_id,pattern=EXCLUDED.pattern,sources=EXCLUDED.sources,reviewed_on=EXCLUDED.reviewed_on,scope=EXCLUDED.scope,notes=EXCLUDED.notes,enabled=EXCLUDED.enabled",
                    &[&format.id,&format.processor_id,&format.pattern,&format.sources,&format.reviewed_on,&format.scope,&format.notes,&format.enabled])?;
            }
            if dry_run { tx.rollback()?; } else {
                tx.commit()?;
            }
            Ok(())
        })
    }
}
