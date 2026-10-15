//! One device-local discovery request shared by pairing surfaces.
use crate::{Core, CoreEvent};
use gpui::{Context, Task};
#[derive(Default)]
pub struct PairingTargets {
    pub distros: Vec<String>,
    pub loading: bool,
    pub error: Option<String>,
    loaded: bool,
    task: Option<Task<()>>,
}
impl Core {
    pub fn pairing_targets(&self) -> &PairingTargets {
        &self.pairing_targets
    }
    pub fn load_pairing_targets(&mut self, retry: bool, cx: &mut Context<Self>) {
        let state = &mut self.pairing_targets;
        if state.loading || (!retry && (state.loaded || state.error.is_some())) {
            return;
        }
        state.loading = true;
        state.error = None;
        state.task = Some(cx.spawn(async move |this, cx| {
            let result = crate::hosts::wsl_distros().await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.pairing_targets;
                state.task = None;
                state.loading = false;
                match result {
                    Ok(distros) => {
                        state.distros = distros;
                        state.loaded = true;
                    }
                    Err(error) => state.error = Some(format!("{error:#}")),
                }
                cx.emit(CoreEvent::HostsChanged);
                cx.notify();
            });
        }));
        cx.notify();
    }
}
