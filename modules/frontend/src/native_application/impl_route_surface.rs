//! Route surface mounting for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-5 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    /// Renders the route content for every route, mounting each
    /// route-port screen on first entry (or when the route identity changes)
    /// and reusing it afterwards.
    #[expect(
        clippy::too_many_lines,
        reason = "one exhaustive dispatch keeps every route visible in a single reviewable match"
    )]
    pub(super) fn route_surface(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        #[cfg(feature = "flight-recorder")]
        let _trace = artisan_tracing::span!("ui", "route_surface");
        match self.route().clone() {
            NativeRoute::Onboarding => {
                if self.onboarding_screen.is_none() {
                    let theme = self.theme;
                    let entries = HarnessCatalog::new()
                        .cards()
                        .iter()
                        .map(|card| {
                            OnboardingHarnessEntry::new(
                                card.clone(),
                                HarnessSetupState::new(
                                    HarnessSetupAction::default(),
                                    false,
                                    false,
                                    "Unavailable",
                                    None,
                                    None,
                                ),
                            )
                        })
                        .collect();
                    self.onboarding_screen =
                        Some(cx.new(move |_| OnboardingScreen::new(theme, entries)));
                }
                self.onboarding_screen
                    .clone()
                    .expect("onboarding screen mounted")
                    .into_any_element()
            }
            NativeRoute::Thread { project, thread } => {
                let key = Some((thread.clone(), self.conversation_host.is_some()));
                if self.thread_screen_key != key || self.thread_screen.is_none() {
                    // A fresh screen starts on its loading gate with an empty
                    // inspector; the live sync below opens the gate with the
                    // thread's history readiness and fills the inspector for
                    // this thread before the first paint, so no fact of the
                    // previous thread's screen can carry over.
                    let mounted = match self.conversation_host.clone() {
                        Some(host) => {
                            let composer = self.composer.clone();
                            let screen = cx.new(|screen_cx| {
                                ThreadScreen::new(host, composer, ThemeMode::Dark, screen_cx)
                            });
                            // Typed text reaches the composer only while it
                            // holds window focus (the platform registers its
                            // text handler for the focused handle, and
                            // unfocused keystrokes are silently swallowed).
                            // Focus it once per opened screen so a fresh
                            // thread is immediately typeable; later renders
                            // must not steal focus back.
                            let focus = self.composer.read(cx).focus_handle(cx);
                            window.focus(&focus, cx);
                            Some(screen)
                        }
                        None => ThreadScreen::mount(thread.clone(), ThemeMode::Dark, cx).ok(),
                    };
                    self.thread_screen = mounted;
                    self.thread_screen_key = key;
                }
                // A mounted thread presents only whole: its transcript
                // snapshot and its tool/reasoning history must both be in.
                // Until then the cold-load gate holds, so tool groups never
                // pop in under already-painted prose and a `Thought for`
                // label never flips to `Worked for` once its tools arrive.
                if let (Some(screen), Some(host)) =
                    (self.thread_screen.clone(), self.conversation_host.clone())
                {
                    let presentable = self.selected_thread.as_ref() == Some(&thread)
                        && self.selected_history_current()
                        && host.read(cx).has_snapshot();
                    let gate = if presentable {
                        ThreadScreenGate::Open
                    } else {
                        ThreadScreenGate::Loading
                    };
                    screen.update(cx, |screen, screen_cx| {
                        if screen.set_gate(gate) {
                            screen_cx.notify();
                        }
                    });
                }
                // Live presentation sync on every render (not just on mount):
                // the content width (window minus the live sidebar, both in
                // logical pixels) drives the inspector fit in both resize
                // directions. The change-guarded setter keeps this free of
                // notify loops; the conversation title lives in the titlebar
                // header, not on this screen.
                if let Some(screen) = self.thread_screen.clone() {
                    let sidebar_width = f32::from(self.desktop_shell_style(window).sidebar_width);
                    let content_width_px = f32::from(window.bounds().size.width) - sidebar_width;
                    screen.update(cx, |screen, screen_cx| {
                        if screen.set_content_width(content_width_px) {
                            screen_cx.notify();
                        }
                    });
                    self.sync_thread_inspector(&screen, &project, &thread, cx);
                }
                // Not a cached view: the syncs above push this frame's gate,
                // width, and inspector facts while the frame is drawing, and a
                // notify made mid-draw neither dirties a cached view for this
                // frame nor schedules another, so a cached screen would paint
                // them late. The transcript inside it is the cached region.
                self.thread_screen.clone().map_or_else(
                    || status_panel(&self.theme, &self.state).into_any_element(),
                    gpui::IntoElement::into_any_element,
                )
            }
            NativeRoute::Editor {
                project, thread, ..
            } => {
                let key = Some((project.clone(), thread.clone(), None));
                if self.editor_screen_key != key || self.editor_screen.is_none() {
                    let display_name = self
                        .project_options
                        .iter()
                        .find(|option| option.id == project)
                        .map_or_else(|| "Workspace".to_owned(), |option| option.name.to_string());
                    let identity = EditorScreenIdentity::new(
                        project.clone(),
                        display_name,
                        thread,
                        None,
                        None,
                    );
                    let screen = EditorScreen::new(
                        identity,
                        Vec::new(),
                        EditorSurfaceState::NoFile { recent: Vec::new() },
                        EditorViewState::default(),
                        self.theme,
                    );
                    self.editor_screen = Some(cx.new(|_| screen));
                    self.editor_screen_key = key;
                }
                self.editor_screen
                    .clone()
                    .expect("editor screen mounted")
                    .into_any_element()
            }
            NativeRoute::Settings { section, engine } => {
                let key = Some((section, engine.clone()));
                if self.settings_screen_key != key || self.settings_screen.is_none() {
                    let screen = cx.new(|screen_cx| {
                        SettingsScreen::new(section, engine.clone(), ThemeMode::Dark, screen_cx)
                    });
                    // The rail enumerates the manifest harness identities so
                    // every real catalog engine is reachable from the normal
                    // Models entry point; fixture identities never enter
                    // production navigation.
                    let entries = self
                        .model_selector
                        .read(cx)
                        .state()
                        .snapshot()
                        .manifest
                        .harnesses
                        .iter()
                        .map(|harness| SettingsEngineNavEntry {
                            id: harness.id.clone(),
                            label: harness.label.clone(),
                        })
                        .collect::<Vec<_>>();
                    screen.update(cx, |screen, screen_cx| {
                        screen.set_engines(entries, screen_cx);
                    });
                    // A real catalog engine mounts its live page; anything
                    // else keeps the legacy surface (including the fixture
                    // route, which stays out of the rail above).
                    if section == SettingsRoute::Engines
                        && let Some(engine_id) = engine.clone()
                        && engine_id != crate::native_settings::FIXTURE_ENGINE_ID
                    {
                        let snapshot = self.settings_engine_snapshot(&engine_id, cx);
                        let known = self
                            .model_selector
                            .read(cx)
                            .state()
                            .snapshot()
                            .manifest
                            .harness(&engine_id)
                            .is_some();
                        let label = self
                            .model_selector
                            .read(cx)
                            .state()
                            .snapshot()
                            .manifest
                            .harness(&engine_id)
                            .map(|harness| harness.label.clone());
                        screen.update(cx, |screen, screen_cx| {
                            screen.set_engine_known(known, screen_cx);
                            screen.set_engine_label(label, screen_cx);
                            if known {
                                screen.set_engine_snapshot(snapshot, screen_cx);
                            }
                        });
                    }
                    let subscription = cx.subscribe(&screen, Self::handle_settings_screen_event);
                    self.settings_screen_subscription = Some(subscription);
                    self.settings_screen = Some(screen);
                    self.settings_screen_key = key;
                }
                self.settings_screen
                    .clone()
                    .expect("settings screen mounted")
                    .into_any_element()
            }
            NativeRoute::NewThread { .. } => self
                .new_thread_surface_section(window, cx)
                .into_any_element(),
        }
    }

    /// Publishes the open thread's inspector facts on every render: the
    /// Project row, the Machine and Branch rows' environment, and the
    /// Checklist.
    ///
    /// Each fact is read from state the application already retains, so it
    /// follows that state live rather than freezing at mount:
    ///
    /// - Project names `project` exactly as the titlebar header does: the
    ///   inspected repository's qualified `owner/repository` label when its
    ///   default remote is browsable, the project's own display name
    ///   otherwise.
    /// - Machine names the connected host exactly as the sidebar's profile
    ///   footer does: the host's registered name (`machine_label`), never the
    ///   computer's raw hostname. It is published as the snapshot's current
    ///   machine, so the row never falls back to the hostname or infers `Not
    ///   connected` from an empty default. Branch comes from the selected
    ///   project's `QueryProjectRepository` reply.
    /// - Checklist is the thread's latest plan update from its engine
    ///   observations.
    /// - Agents are the subagents the thread's live run has started, read
    ///   from the same observations ([`thread_agent_rows`]); a settled run
    ///   lists none.
    ///
    /// Every fact is fenced to the route's own `project` and `thread`:
    /// repository facts retained (or requested) for another project, and
    /// observations retained for another thread, publish nothing, so a
    /// switch never shows the previous thread's project, branch, checklist,
    /// or agents. The setters are change-guarded, and the checklist and the
    /// agents are compared borrowed before any entry is copied, so an
    /// unchanged frame notifies nothing and copies no entry.
    fn sync_thread_inspector(
        &self,
        screen: &Entity<ThreadScreen>,
        project: &ProjectId,
        thread: &ThreadId,
        cx: &mut Context<Self>,
    ) {
        let repository_current = self.titlebar_repository_project.as_ref() == Some(project);
        let project_label = self
            .project_options
            .iter()
            .find(|option| &option.id == project)
            .map(|option| {
                self.titlebar_repository
                    .as_ref()
                    .filter(|_| repository_current)
                    .map_or_else(
                        || option.name.to_string(),
                        TitlebarRepository::qualified_label,
                    )
            });
        let environment = ThreadEnvironmentInput {
            machines: Some(HostMachinesSnapshot::new(vec![HostMachineSnapshot::new(
                self.machine_home.as_deref().map_or_else(
                    || self.machine_label.clone(),
                    |home| home.to_string_lossy().into_owned(),
                ),
                HostMachineKind::Local,
                self.machine_label.clone(),
            )])),
            repository: self
                .project_repository
                .clone()
                .filter(|_| repository_current),
            ..ThreadEnvironmentInput::default()
        };
        let observations = self
            .engine_observations
            .as_ref()
            .filter(|observations| observations.thread_id() == thread);
        let plan = observations
            .and_then(EngineObservationState::latest_plan)
            .map_or(&[][..], |plan| plan.entries().as_slice());
        // Agents belong to a live run: once it settles they are done and the
        // section clears instead of listing finished work indefinitely.
        let agents = observations
            .filter(|_| self.run_controls.is_live_for(thread))
            .map(thread_agent_rows)
            .unwrap_or_default();
        screen.update(cx, |screen, screen_cx| {
            let mut changed = screen.set_project_label(project_label);
            changed |= screen.set_environment(environment);
            if !screen.agents_present(&agents) {
                changed |=
                    screen.set_agents(agents.iter().copied().map(thread_agent_entry).collect());
            }
            if !screen.checklist_presents(plan) {
                changed |= screen.set_checklist(
                    plan.iter()
                        .map(ThreadChecklistEntry::from_plan_entry)
                        .collect(),
                );
            }
            if changed {
                screen_cx.notify();
            }
        });
    }
}
