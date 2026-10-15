//! Each host owns one request per lazy catalog, shared by every visible consumer.
use super::{HostData, HostEvent};
use gpui::{Context, Task};

#[derive(Default)]
pub struct ProjectionLoad {
    pub loading: bool,
    pub error: Option<String>,
    task: Option<Task<()>>,
}
impl HostData {
    pub fn prompt_load(&self) -> &ProjectionLoad {
        &self.prompt_load
    }
    pub fn catalog_load(&self) -> &ProjectionLoad {
        &self.catalog_load
    }

    pub fn load_prompt(&mut self, retry: bool, cx: &mut Context<Self>) {
        if !self.is_live()
            || self.prompt_load.loading
            || (!retry && (self.global_prompt.is_some() || self.prompt_load.error.is_some()))
        {
            return;
        }
        self.prompt_load.loading = true;
        self.prompt_load.error = None;
        let (client, generation, revision) =
            (self.client.clone(), self.generation, self.prompt_revision);
        self.prompt_load.task = Some(cx.spawn(async move |this, cx| {
            let result = client.global_prompt().await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation || this.prompt_revision != revision {
                    return;
                }
                this.prompt_load = ProjectionLoad::default();
                match result {
                    Ok(prompt) => {
                        let expected = this
                            .settings
                            .get("global_prompt_version")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0);
                        if prompt.version < expected {
                            this.prompt_load.error = Some(
                                "Forge returned an older prompt version. Retry to refresh.".into(),
                            );
                            cx.emit(HostEvent::Changed);
                            cx.notify();
                        } else {
                            this.remember_prompt(prompt, cx);
                        }
                    }
                    Err(error) => {
                        this.prompt_load.error =
                            Some(format!("Couldn't read the global prompt: {error:#}"));
                        cx.emit(HostEvent::Changed);
                        cx.notify();
                    }
                }
            });
        }));
        cx.emit(HostEvent::Changed);
        cx.notify();
    }
    pub fn load_skill_catalog(&mut self, retry: bool, cx: &mut Context<Self>) {
        if !self.is_live()
            || self.catalog_load.loading
            || (!retry && (self.skill_catalog.is_some() || self.catalog_load.error.is_some()))
        {
            return;
        }
        self.catalog_load.loading = true;
        self.catalog_load.error = None;
        let (client, generation, revision) =
            (self.client.clone(), self.generation, self.catalog_revision);
        self.catalog_load.task = Some(cx.spawn(async move |this, cx| {
            let result = client.skill_catalog().await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation || this.catalog_revision != revision {
                    return;
                }
                this.catalog_load = ProjectionLoad::default();
                match result {
                    Ok(catalog) => this.remember_skill_catalog(catalog, cx),
                    Err(error) => {
                        this.catalog_load.error = Some(format!("Couldn't list skills: {error:#}"));
                        cx.emit(HostEvent::Changed);
                        cx.notify();
                    }
                }
            });
        }));
        cx.emit(HostEvent::Changed);
        cx.notify();
    }
    pub(super) fn cancel_projection_reads(&mut self) {
        self.prompt_load = ProjectionLoad::default();
        self.catalog_load = ProjectionLoad::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConnectionStatus, ForgeClient, HostId};
    use gpui::{AppContext as _, Entity, TestAppContext};
    fn failed_host(cx: &mut TestAppContext) -> Entity<HostData> {
        let identity = artisan_api::tls::Identity::generate("projection-test").unwrap();
        let fingerprint =
            artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
        cx.new(|cx| {
            let mut host = HostData::new(HostId("test".into()), "test".into(), client, cx);
            host._connection.abort();
            host._follow = gpui::Task::ready(());
            host.status = ConnectionStatus::Live;
            host
        })
    }
    #[gpui::test]
    fn shared_reads_surface_failure_and_only_explicit_retry_starts_again(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let host = failed_host(cx);
        host.update(cx, |host, cx| {
            host.load_prompt(false, cx);
            host.load_prompt(false, cx);
            host.load_skill_catalog(false, cx);
            host.load_skill_catalog(false, cx);
            assert!(host.prompt_load.loading && host.catalog_load.loading);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if host.read_with(cx, |host, _| {
                host.prompt_load.error.is_some() && host.catalog_load.error.is_some()
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "both refused reads must finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        host.update(cx, |host, cx| {
            host.load_prompt(false, cx);
            host.load_skill_catalog(false, cx);
            assert!(
                !host.prompt_load.loading && !host.catalog_load.loading,
                "ordinary render-driven reads must not loop on failure"
            );
            host.load_prompt(true, cx);
            host.load_skill_catalog(true, cx);
            assert!(host.prompt_load.loading && host.catalog_load.loading);
            assert!(host.prompt_load.error.is_none() && host.catalog_load.error.is_none());
            host.cancel_projection_reads();
        });
    }
}
