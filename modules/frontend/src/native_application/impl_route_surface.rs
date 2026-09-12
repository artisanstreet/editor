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
            NativeRoute::Thread { thread, .. } => {
                let key = Some((thread.clone(), self.conversation_host.is_some()));
                if self.thread_screen_key != key || self.thread_screen.is_none() {
                    // The environment card must never infer a disconnect from
                    // its empty default: feed the readily existing profile
                    // hostname (the same authoritative identity behind the
                    // sidebar) so Machine names this computer instead of
                    // reporting `Not connected` while connected.
                    let environment = ThreadEnvironmentInput {
                        identity: self.profile_hostname.clone().map(HostIdentitySnapshot::new),
                        ..ThreadEnvironmentInput::default()
                    };
                    let mounted = match self.conversation_host.clone() {
                        Some(host) => {
                            let composer = self.composer.clone();
                            let screen = cx.new(|screen_cx| {
                                ThreadScreen::new(host, composer, ThemeMode::Dark, screen_cx)
                            });
                            screen.update(cx, |screen, _| {
                                screen.set_gate(ThreadScreenGate::Open);
                                screen.set_environment(environment.clone());
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
                        None => ThreadScreen::mount(thread.clone(), ThemeMode::Dark, cx)
                            .ok()
                            .inspect(|screen| {
                                screen.update(cx, |screen, _| {
                                    screen.set_environment(environment);
                                });
                            }),
                    };
                    self.thread_screen = mounted;
                    self.thread_screen_key = key;
                }
                // Live presentation sync on every render (not just on mount):
                // the content width (window minus the live sidebar, both in
                // logical pixels) drives the inspector fit in both resize
                // directions. The change-guarded setter keeps this free of
                // notify loops; the conversation title lives in the titlebar
                // header, not on this screen.
                if let Some(screen) = self.thread_screen.clone() {
                    let sidebar_width = f32::from(
                        DesktopShellStyle::resolve(self.sidebar_collapsed, window.scale_factor())
                            .sidebar_width,
                    );
                    let content_width_px = f32::from(window.bounds().size.width) - sidebar_width;
                    screen.update(cx, |screen, screen_cx| {
                        if screen.set_content_width(content_width_px) {
                            screen_cx.notify();
                        }
                    });
                }
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
}
