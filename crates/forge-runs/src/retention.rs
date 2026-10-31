use crate::Hub;
use anyhow::{Context, Result};
use artisan_api::{THREAD_RETENTION_DAYS_KEY, Thread};
use std::collections::HashSet;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const BATCH: usize = 8;

impl Hub {
    pub async fn remove_inactive_threads(&self) -> Result<usize> {
        let Some(days) = self.retention_days().await? else {
            return Ok(0);
        };
        let cutoff = artisan_api::now().saturating_sub(i64::from(days).saturating_mul(DAY_MS));
        let mut total = 0;
        loop {
            let live: HashSet<_> = self.lock().keys().cloned().collect();
            let removed = self
                .store()
                .remove_inactive_threads(cutoff, live, BATCH)
                .await?;
            if removed.is_empty() {
                break;
            }
            total += removed.len();
            for thread in removed {
                self.forget_removed_thread(thread).await;
            }
        }
        if total > 0 {
            tracing::info!(removed = total, days, "removed inactive threads");
            self.store()
                .gc_blobs()
                .await
                .context("collect images of removed threads")?;
        }
        Ok(total)
    }

    async fn retention_days(&self) -> Result<Option<u32>> {
        let value = self
            .store()
            .setting(THREAD_RETENTION_DAYS_KEY)
            .await?
            .or_else(|| {
                artisan_api::setting_decl(THREAD_RETENTION_DAYS_KEY)
                    .map(artisan_api::SettingDecl::default_value)
            })
            .unwrap_or(serde_json::Value::Null);
        Ok(value
            .as_u64()
            .and_then(|days| u32::try_from(days).ok())
            .filter(|days| *days > 0))
    }

    async fn forget_removed_thread(&self, thread: Thread) {
        let suggested = self.suggestions_lock().remove(&thread.id);
        if let Some(suggested) = suggested {
            suggested.cancel.cancel();
        }
        self.attention_lock().remove(&thread.id);
        self.untitled_lock().remove(&thread.id);
        if !thread.project_id.is_scratch() {
            return;
        }
        let root = self.scratch_root();
        let id = thread.id.clone();
        let removed = tokio::task::spawn_blocking(move || {
            crate::scratch::remove_scratch_folder(&root, &thread)
        })
        .await;
        match removed {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(thread = %id, "remove scratch folder of removed thread: {error:#}");
            }
            Err(error) => {
                tracing::warn!(thread = %id, "remove scratch folder task of removed thread: {error}");
            }
        }
    }
}
