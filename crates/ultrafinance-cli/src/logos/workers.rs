//! Keep only the configured number of website jobs in flight, even for a whole market.
use anyhow::{Context, Result, ensure};
use std::future::Future;
use tokio::task::JoinSet;

pub async fn run<I, F, Fut, T>(
    items: I,
    workers: usize,
    mut job: F,
    mut report: impl FnMut(T) -> Result<()>,
) -> Result<()>
where
    I: IntoIterator,
    F: FnMut(I::Item) -> Fut,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    ensure!(workers > 0, "workers must be positive");
    let mut items = items.into_iter();
    let mut active = JoinSet::new();
    loop {
        while active.len() < workers {
            let Some(item) = items.next() else {
                break;
            };
            active.spawn(job(item));
        }
        let Some(result) = active.join_next().await else {
            break;
        };
        report(result.context("logo discovery worker failed")?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::{Semaphore, mpsc};

    #[tokio::test]
    async fn fills_worker_slots_and_refills_only_when_one_completes() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let permits = Arc::new(Semaphore::new(0));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let (done_tx, mut done_rx) = mpsc::unbounded_channel();
        let gate = permits.clone();
        let running = active.clone();
        let highest = peak.clone();
        let scheduler = tokio::spawn(async move {
            run(
                0..5,
                2,
                move |item| {
                    let active = running.clone();
                    let peak = highest.clone();
                    let permits = gate.clone();
                    let started = started_tx.clone();
                    async move {
                        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(count, Ordering::SeqCst);
                        started.send(item).unwrap();
                        permits.acquire().await.unwrap().forget();
                        active.fetch_sub(1, Ordering::SeqCst);
                        item
                    }
                },
                |item| {
                    done_tx.send(item).unwrap();
                    Ok(())
                },
            )
            .await
        });
        for _ in 0..2 {
            tokio::time::timeout(std::time::Duration::from_secs(5), started_rx.recv())
                .await
                .unwrap()
                .unwrap();
        }
        assert_eq!(active.load(Ordering::SeqCst), 2);
        assert!(started_rx.try_recv().is_err());
        // One completion releases exactly one slot for the next website.
        permits.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(5), done_rx.recv())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 2);
        assert!(started_rx.try_recv().is_err());
        permits.add_permits(4);
        tokio::time::timeout(std::time::Duration::from_secs(5), scheduler)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let mut completed = vec![];
        while let Some(item) = done_rx.recv().await {
            completed.push(item);
        }
        assert_eq!(completed.len(), 4);
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn one_worker_preserves_order_and_item_errors_do_not_stop_other_jobs() {
        let mut results = vec![];
        run(
            0..4,
            1,
            |item| async move { if item == 1 { Err(item) } else { Ok(item) } },
            |result| {
                results.push(result);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(results, vec![Ok(0), Err(1), Ok(2), Ok(3)]);
    }
}
