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
