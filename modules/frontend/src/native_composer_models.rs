use super::*;
use crate::native_model_catalog::NativeOptionValue;
use crate::native_model_selector::{NativeModelSelectorEvent, NativeModelSelectorStatus};

impl NativeApplication {
    pub(super) fn handle_composer_model_event(
        &mut self,
        event: &NativeModelSelectorEvent,
        cx: &mut Context<Self>,
    ) {
        let NativeModelSelectorEvent::SelectPolicy(policy) = event else {
            return;
        };
        if self.selected_thread.is_none()
            || self.engine_settings.pending_save_request_id().is_some()
        {
            return;
        }
        let catalog = self.model_selector.read(cx).state().snapshot().clone();
        let outcome = crate::composer_model_config::config_for_policy(
            &catalog,
            policy,
            self.engine_settings.authoritative_config(),
        );
        match outcome {
            Ok(config) => {
                *self.engine_settings.draft_mut() =
                    crate::engine_settings::EngineSettingsDraft::from_config(&config);
                if self.engine_settings.can_save() {
                    self.save_engine_settings(cx);
                    self.model_selector.update(cx, |selector, cx| {
                        selector.set_status(
                            NativeModelSelectorStatus {
                                saving: self.engine_settings.pending_save_request_id().is_some(),
                                error: None,
                                authoritative: false,
                            },
                            cx,
                        )
                    });
                } else {
                    self.sync_composer_model_policy(cx);
                }
            }
            Err(message) => self.model_selector.update(cx, |selector, cx| {
                selector.set_status(
                    NativeModelSelectorStatus {
                        saving: false,
                        error: Some(message.to_owned()),
                        authoritative: false,
                    },
                    cx,
                )
            }),
        }
    }

    pub(super) fn sync_composer_model_policy(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.model_selector.read(cx).state().snapshot();
        let policy = self
            .engine_settings
            .authoritative_config()
            .and_then(|config| {
                let native = config.selection().as_opencode2();
                if snapshot.scope.as_ref()?.profile_id != native.profile_id().as_str() {
                    return None;
                }
                let model = snapshot.manifest.models.iter().find(|model| {
                    model.native_selection.as_ref().is_some_and(|selection| {
                        selection.model_id == native.model_id().as_str()
                            && selection.provider_route_id == native.route_id().as_str()
                            && selection.variant_id.as_deref()
                                == native.variant_id().map(|v| v.as_str())
                    })
                })?;
                let mut policy = snapshot.preview_policy_for_model(&model.id).ok()?;
                let option = snapshot
                    .manifest
                    .harness(&policy.engine_id)?
                    .permissions
                    .options
                    .iter()
                    .find(|option| option.id == native.permission().permission_id().as_str())?;
                policy.permission = Some(NativeOptionValue {
                    id: option.id.clone(),
                    native_value: option.native_value.clone(),
                });
                Some(policy)
            });
        self.model_selector.update(cx, |selector, cx| {
            selector.set_policy(policy, cx);
            selector.set_status(
                NativeModelSelectorStatus {
                    saving: false,
                    error: None,
                    authoritative: true,
                },
                cx,
            );
        });
    }
}
