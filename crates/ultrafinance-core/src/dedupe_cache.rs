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
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn checkpoints_reopen_recover_torn_tails_and_reject_invalid_answers() -> Result<()> {
        let path = std::env::temp_dir().join(format!("ultra-cache-{}.jsonl", uuid::Uuid::new_v4()));
        let q = json!({"evidence":"public identity"});
        let digest = Cache::key("https://provider.test", "model", &q)?;
        let answer = json!({"type":"choice","choice":"different","confidence":0.99,"probabilities":{"same":0.0,"different":1.0,"related":0.0,"insufficient":0.0}});
        let mut cache = Cache::open(&path)?;
        assert!(
            cache
                .save(vec![(digest.clone(), json!({"choice":"same"}))])
                .is_err()
        );
        cache.save(vec![(digest.clone(), answer.clone())])?;
        drop(cache);
        OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"{\"torn\":")?;
        let mut cache = Cache::open(&path)?;
        assert_eq!(cache.get(&digest), Some(answer.clone()));
        assert!(
            cache
                .get(&Cache::key("https://other.test", "model", &q)?)
                .is_none()
        );
        assert!(
            cache
                .get(&Cache::key("https://provider.test", "other-model", &q)?)
                .is_none()
        );
        assert!(
            cache
                .get(&Cache::key(
                    "https://provider.test",
                    "model",
                    &json!({"evidence":"changed"})
                )?)
                .is_none()
        );
        cache.save(vec![(digest.clone(), answer.clone())])?;
        drop(cache);
        assert_eq!(Cache::open(&path)?.get(&digest), Some(answer));
        std::fs::remove_file(path)?;
        Ok(())
    }
}
