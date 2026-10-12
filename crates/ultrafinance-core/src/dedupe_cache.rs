//! Opt-in local journal of validated provider answers, keyed by exact evidence.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{BufRead, BufReader, Write},
    path::Path,
};

#[derive(Serialize, Deserialize)]
struct Entry {
    version: u32,
    digest: String,
    answer: Value,
}
pub(crate) struct Cache {
    file: std::fs::File,
    answers: HashMap<String, Value>,
}
impl Cache {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).read(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .context("cannot open dedupe answer checkpoint")?;
        let mut reader = BufReader::new(file.try_clone()?);
        let mut answers = HashMap::new();
        let mut complete_bytes = 0;
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            if !line.ends_with('\n') {
                // A process can stop during an append. Discard only that torn tail.
                file.set_len(complete_bytes)?;
                break;
            }
            let entry: Entry =
                serde_json::from_str(&line).context("invalid dedupe answer checkpoint")?;
            if entry.version != 1
                || entry.digest.len() != 64
                || !entry.digest.bytes().all(|b| b.is_ascii_hexdigit())
            {
                bail!("invalid dedupe answer checkpoint identity");
            }
            super::dedupe::accepted(&entry.answer, 0.5)
                .context("invalid checkpointed Jev answer")?;
            answers.insert(entry.digest, entry.answer);
            complete_bytes += n as u64;
        }
        Ok(Self { file, answers })
    }
    pub(crate) fn key(endpoint: &str, model: &str, question: &Value) -> Result<String> {
        let bytes = serde_json::to_vec(&(1, endpoint, model, question))?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
    pub(crate) fn get(&self, digest: &str) -> Option<Value> {
        self.answers.get(digest).cloned()
    }
    pub(crate) fn save(&mut self, entries: Vec<(String, Value)>) -> Result<()> {
        let mut bytes = Vec::new();
        for (digest, answer) in &entries {
            super::dedupe::accepted(answer, 0.5)?;
            serde_json::to_writer(
                &mut bytes,
                &Entry {
                    version: 1,
                    digest: digest.clone(),
                    answer: answer.clone(),
                },
            )?;
            bytes.push(b'\n');
        }
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        self.answers.extend(entries);
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/dedupe_cache/tests.rs"]
mod tests;
