//! Timestamp overrides used only by the explicit legacy importer and unit fixtures.
//! All writes reuse the canonical transaction operations and normal writer queue.

use crate::{Store, writer::Stamp};
use anyhow::{Context, Result};
use artisan_api::{CreateProject, CreateThread, Project, Run, Thread, ThreadId, UpdateThread};

impl Store {
    pub(crate) async fn create_project_at(
        &self,
        request: CreateProject,
        stamp: Stamp,
    ) -> Result<Project> {
        let id = request.id.clone();
        self.write(move |tx| tx.with_stamp(stamp, |tx| tx.create_project(request)))
            .await
            .with_context(|| format!("create project {id}"))
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::ProjectId;

    #[tokio::test]
    async fn imported_stamp_does_not_escape_its_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let request = CreateProject {
            id: ProjectId::new(),
            path: "/imported".into(),
            name: None,
        };
        let id = request.id.clone();
        let imported = store
            .create_project_at(
                request,
                Stamp {
                    created_at: 10,
                    updated_at: 20,
                },
            )
            .await
            .unwrap();
        assert_eq!(imported.created_at, 10);
        let existing = store
            .create_project(CreateProject {
                id,
                path: "/replacement".into(),
                name: Some("Replacement".into()),
            })
            .await
            .unwrap();
        assert_eq!(
            existing, imported,
            "normal writes retain imported identity and time"
        );
        let runtime = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/runtime".into(),
                name: None,
            })
            .await
            .unwrap();
        assert!(runtime.created_at > 20, "normal writes use the clock");
        store
            .write(|tx| {
                tx.with_stamp(
                    Stamp {
                        created_at: 30,
                        updated_at: 40,
                    },
                    |tx| {
                        let failed: Result<()> = tx.with_stamp(
                            Stamp {
                                created_at: 50,
                                updated_at: 60,
                            },
                            |_| Err(anyhow::anyhow!("fixture failure")),
                        );
                        assert!(failed.is_err());
                        assert_eq!(
                            tx.created_at(),
                            30,
                            "failed nested override restores its caller"
                        );
                        Ok(())
                    },
                )?;
                assert!(
                    tx.created_at() > 60,
                    "override restored before transaction finishes"
                );
                Ok(())
            })
            .await
            .unwrap();
    }
}
