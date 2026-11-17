use crate::{Store, writer::Stamp};
use anyhow::{Context, Result};
use artisan_api::{CreateThread, Run, Thread, ThreadId, UpdateThread};

impl Store {
    pub(crate) async fn create_thread_at(
        &self,
        request: CreateThread,
        stamp: Stamp,
    ) -> Result<Thread> {
        let id = request.id.clone();
        self.write(move |tx| tx.with_stamp(stamp, |tx| tx.create_thread(request)))
            .await
            .with_context(|| format!("create thread {id}"))
    }

    pub(crate) async fn update_thread_at(
        &self,
        id: &ThreadId,
        update: UpdateThread,
        stamp: Stamp,
    ) -> Result<Option<Thread>> {
        let id = id.clone();
        self.write(move |tx| tx.with_stamp(stamp, |tx| tx.update_thread(&id, update)))
            .await
    }

    pub(crate) async fn upsert_run_at(&self, run: Run, stamp: Stamp) -> Result<Run> {
        let (id, thread_id) = (run.id.clone(), run.thread_id.clone());
        self.write(move |tx| tx.with_stamp(stamp, |tx| tx.upsert_run(run)))
            .await
            .with_context(|| format!("save run {id} of thread {thread_id}"))
    }
}
