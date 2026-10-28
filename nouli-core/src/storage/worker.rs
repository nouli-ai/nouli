//! Owned blocking jobs keep SQLite and filesystem waits off async executors.
use super::{Ledger, OpenMode, StorageError};
use std::{path::PathBuf, sync::Arc};
use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("ledger worker stopped before returning a result")]
    Stopped,
}

/// Clones share one serialized connection. A submitted blocking job owns its
/// lock until it finishes, even if the awaiting future is dropped. Cancellation
/// never releases a guard while its database transaction is still running.
#[derive(Clone)]
pub struct LedgerWorker {
    inner: Arc<Mutex<Ledger>>,
}
impl From<Ledger> for LedgerWorker {
    fn from(ledger: Ledger) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ledger)),
        }
    }
}
impl LedgerWorker {
    pub async fn open(root: PathBuf, mode: OpenMode) -> Result<Self, WorkerError> {
        Ok(
            tokio::task::spawn_blocking(move || Ledger::open(&root, mode))
                .await
                .map_err(|_| WorkerError::Stopped)??
                .into(),
        )
    }
    pub async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Ledger) -> T + Send + 'static,
    ) -> Result<T, WorkerError> {
        let mut ledger = self.inner.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || operation(&mut ledger))
            .await
            .map_err(|_| WorkerError::Stopped)
    }
    #[cfg(test)]
    pub(crate) fn inspect<T>(&self, operation: impl FnOnce(&mut Ledger) -> T) -> T {
        operation(
            &mut self
                .inner
                .try_lock()
                .expect("test inspection follows completed jobs"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{history::ModelSelection, identity::Id, storage::guards::GuardError};
    #[tokio::test]
    async fn dropped_awaiter_cannot_release_a_guard_owned_by_an_active_job() {
        let root = std::env::temp_dir().join(format!("nouli-worker-{}", Id::generate()));
        let worker = LedgerWorker::open(root.clone(), OpenMode::ReadWriteCreate)
            .await
            .unwrap();
        let other = LedgerWorker::open(root.clone(), OpenMode::ReadWriteCreate)
            .await
            .unwrap();
        let session = worker
            .call(|ledger| ledger.create_session(None, ModelSelection::default()))
            .await
            .unwrap()
            .unwrap();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let running = worker.clone();
        let task = tokio::spawn(async move {
            running
                .call(move |ledger| {
                    let _guard = ledger.session_guard(session.id).unwrap();
                    ready_tx.send(()).unwrap();
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                })
                .await
        });
        ready_rx.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let locked = other
            .call(move |ledger| {
                matches!(
                    ledger.session_guard(session.id),
                    Err(GuardError::WriterActive)
                )
            })
            .await
            .unwrap();
        release_tx.send(()).unwrap();
        worker.call(|_| ()).await.unwrap();
        assert!(locked);
        assert!(
            other
                .call(move |ledger| ledger.session_guard(session.id).is_ok())
                .await
                .unwrap()
        );
        drop(worker);
        drop(other);
        std::fs::remove_dir_all(root).unwrap();
    }
}
