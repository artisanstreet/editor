use crate::error::Staged;
use crate::writer::{Stamp, UpdateThreadFailure, WriteFailure};
use crate::{CreateThreadError, Store};
use artisan_api::{CreateThread, Run, Thread, ThreadId, UpdateThread};

impl Store {
    pub(crate) async fn create_thread_at(
        &self,
        request: CreateThread,
        stamp: Stamp,
    ) -> Result<Thread, CreateThreadError> {
        let thread = request.id.clone();
        self.write(move |tx| {
            tx.with_stamp(stamp, |tx| tx.create_thread(request))
                .map_err(Staged::Failed)
        })
        .await
        .map_err(|staged| match staged {
            Staged::Failed(error) => error,
            Staged::Storage(source) => CreateThreadError::Storage { thread, source },
        })
    }

    pub(crate) async fn update_thread_at(
        &self,
        id: &ThreadId,
        update: UpdateThread,
        stamp: Stamp,
    ) -> Result<Option<Thread>, UpdateThreadFailure> {
        let id = id.clone();
        self.write(move |tx| tx.with_stamp(stamp, |tx| tx.update_thread(&id, update)))
            .await
    }

    pub(crate) async fn upsert_run_at(&self, run: Run, stamp: Stamp) -> Result<Run, WriteFailure> {
        self.write(move |tx| tx.with_stamp(stamp, |tx| tx.upsert_run(run)))
            .await
    }
}
