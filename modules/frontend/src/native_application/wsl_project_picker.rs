//! Windows owns the dialog; Forge owns canonicalization and directory admission.
use super::*;

impl NativeApplication {
    pub(super) fn choose_wsl_project(&mut self, distribution: String, cx: &mut Context<Self>) {
        if self.intake_restore_state.is_none() {
            self.intake_restore_state = Some(self.state.clone());
        }
        self.intake_stage = Some(NativeProjectIntakeStage::PickingDirectory);
        self.state = NativeViewState::Loading;
        self.set_picker_disabled(true, cx);
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let home =
                        crate::native_hosts::wsl::default_home_share(&distribution).ok_or(())?;
                    native_dialog::FileDialogBuilder::default()
                        .set_title("Choose a project folder in WSL")
                        .set_location(&home)
                        .open_single_dir()
                        .show()
                        .map_err(|_| ())?
                        .map(|path| {
                            crate::native_hosts::wsl::linux_path(
                                &path.to_string_lossy(),
                                &distribution,
                            )
                            .ok_or(())
                        })
                        .transpose()
                })
                .await;
            let _ = view.update(cx, |app, cx| match result {
                Ok(Some(path)) => {
                    if let Err(error) =
                        app.submit_command(NativeTransportCommand::BeginProjectIntakeAt(path))
                    {
                        app.handle_intake_failed(
                            NativeProjectIntakeOperation::PickDirectory,
                            command_failure(error),
                            false,
                            cx,
                        );
                    }
                }
                Ok(None) => app.handle_intake_cancelled(cx),
                Err(()) => app.handle_intake_failed(
                    NativeProjectIntakeOperation::PickDirectory,
                    ServiceFailure {
                        stage: ServiceFailureStage::Request,
                        category: ServiceFailureCategory::InvalidConfiguration,
                    },
                    false,
                    cx,
                ),
            });
        })
        .detach();
    }
}
