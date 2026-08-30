//! Native-host feasibility window and first workflow-leaf proof surface.
//!
//! The host chrome values below (window size, colors, copy, and click counter)
//! remain feasibility-only proof material for the real Windows GPUI stack.
//! The embedded project picker is the first product-specific native leaf and
//! follows the audited interaction contract in `docs/ui/INVENTORY.md` §6.2.
//! The host also owns the application-level Tab/Shift+Tab traversal route
//! (`bind_proof_actions`): GPUI supplies no automatic Tab binding, so real
//! keyboard reachability of the picker trigger runs through this surface.

use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;

use crate::{conversation_host, project_picker};
use artisan_domain::ThreadId;
use artisan_ui::theme::ThemeMode;
use gpui::{
    App, AppContext as _, Application, Bounds, Context, Entity, FocusHandle, KeyBinding,
    MouseButton, MouseDownEvent, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowOptions, actions, div,
    prelude::{InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _},
    px, rgb, size,
};

// Feasibility-only quit action exercising the `actions!` macro registry, plus
// the proof surface's own application Tab/Shift+Tab traversal route: GPUI has
// no automatic Tab binding, so every real host owns one.
actions!(proof, [Quit, NextTabStop, PreviousTabStop]);

/// Key context scoping the proof host's Tab/Shift+Tab bindings.
const PROOF_KEY_CONTEXT: &str = "artisan-proof";

/// Binds the complete proof-host keymap: feasibility quit plus the
/// application-level Tab/Shift+Tab routing to the window-native traversal
/// engine. Shared verbatim by the real proof entry point and the native
/// probes so reachability coverage executes the actual host semantics.
pub fn bind_proof_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("tab", NextTabStop, Some(PROOF_KEY_CONTEXT)),
        KeyBinding::new("shift-tab", PreviousTabStop, Some(PROOF_KEY_CONTEXT)),
    ]);
}

/// Feasibility-only window title.
const WINDOW_TITLE: &str = "Artisan — phase 1 GPUI proof";
/// Feasibility-only surface width in logical pixels.
const SURFACE_WIDTH: f32 = 640.0;
/// Feasibility-only surface height in logical pixels.
const SURFACE_HEIGHT: f32 = 720.0;
/// Bounded application-side queue for effects awaiting a future adapter.
pub const PROOF_MAX_CONVERSATION_EFFECTS: usize = conversation_host::CONVERSATION_HOST_MAX_EFFECTS;
/// Height reserved for the genuine native conversation child in the proof
/// layout.
const CONVERSATION_HEIGHT: f32 = 360.0;
/// Feasibility-only background color.
const BACKGROUND: u32 = 0x10_14_18;
/// Feasibility-only foreground color.
const FOREGROUND: u32 = 0xE8_EA_ED;
/// Feasibility-only muted caption color.
const MUTED: u32 = 0x8A_93_9E;

/// Minimal interactive entity rendered inside the proof window.
pub struct ProofSurface {
    focus_handle: FocusHandle,
    clicks: usize,
    /// The native project-picker leaf hosted by this feasibility surface.
    picker: Entity<project_picker::ProjectPickerView>,
    /// The genuine controller/surface host mounted beside the picker.
    conversation_host: Option<Entity<conversation_host::ConversationHost>>,
    /// Keeps the proof-level adapter subscribed for the full proof lifetime.
    _conversation_host_subscription: Option<Subscription>,
    /// Typed, bounded application boundary for host effects. No transport or
    /// window action is executed by this proof surface.
    conversation_effects: Vec<conversation_host::ConversationHostEffect>,
}

impl ProofSurface {
    /// Builds the proof surface exactly as the real entry point does:
    /// focused root, fresh click counter, and a picker leaf over the proof
    /// catalog with the first attachment current. Public so native probes
    /// mount the genuine host rather than a copy of its wiring.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window);
        let (projects, current) = proof_catalog();
        let picker = cx.new(|picker_cx| {
            project_picker::ProjectPickerView::new(projects, current, ThemeMode::Dark, picker_cx)
        });
        let mut conversation_effects = Vec::with_capacity(PROOF_MAX_CONVERSATION_EFFECTS);
        let (conversation_host, conversation_host_subscription) =
            match ThreadId::parse("proof-conversation") {
                Ok(thread_id) => match conversation_host::ConversationHost::mount(
                    thread_id,
                    ThemeMode::Dark,
                    &mut *cx,
                ) {
                    Ok(host) => {
                        suppress_conversation_tab_stops(&host, cx);
                        let subscription = cx.observe(&host, |proof, host, cx| {
                            proof.collect_conversation_effects(host, cx);
                        });
                        (Some(host), Some(subscription))
                    }
                    Err(error) => {
                        conversation_effects.push(
                            conversation_host::ConversationHostEffect::Refused {
                                refusal: conversation_host::ConversationHostRefusal::Initialization(
                                    error,
                                ),
                            },
                        );
                        (None, None)
                    }
                },
                Err(error) => {
                    conversation_effects.push(conversation_host::ConversationHostEffect::Refused {
                        refusal: conversation_host::ConversationHostRefusal::Initialization(
                            conversation_host::ConversationHostError::InvalidThreadId(error),
                        ),
                    });
                    (None, None)
                }
            };
        let mut proof = Self {
            focus_handle,
            clicks: 0,
            picker,
            conversation_host,
            _conversation_host_subscription: conversation_host_subscription,
            conversation_effects,
        };
        if let Some(host) = proof.conversation_host.clone() {
            proof.collect_conversation_effects(host, cx);
        }
        proof
    }

    /// The embedded project-picker leaf.
    #[must_use]
    pub fn picker(&self) -> &Entity<project_picker::ProjectPickerView> {
        &self.picker
    }

    /// The genuine conversation host entity, when initialization succeeded.
    #[must_use]
    pub fn conversation_host(&self) -> Option<&Entity<conversation_host::ConversationHost>> {
        self.conversation_host.as_ref()
    }

    /// Effects retained at the actual proof/application boundary.
    #[must_use]
    pub fn pending_conversation_effects(&self) -> &[conversation_host::ConversationHostEffect] {
        &self.conversation_effects
    }

    /// Drains application-bound conversation effects in FIFO order and pumps
    /// the newly available bounded boundary once.
    ///
    /// The context is required because an application drain must explicitly
    /// collect the older host/controller prefix and retry surface actions;
    /// unrelated GPUI notifications are not used as a completion signal. The
    /// pump has two collection passes and one action pass. Any remaining tail
    /// stays observable in its owning host, controller, or surface queue.
    #[must_use]
    pub fn drain_conversation_effects(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<conversation_host::ConversationHostEffect> {
        let effects = std::mem::replace(
            &mut self.conversation_effects,
            Vec::with_capacity(PROOF_MAX_CONVERSATION_EFFECTS),
        );
        if let Some(host) = self.conversation_host.clone() {
            self.pump_conversation_boundary(host, cx);
        }
        effects
    }

    /// The proof surface's own tracked focus handle (its startup focus).
    #[must_use]
    pub fn root_focus(&self) -> &FocusHandle {
        &self.focus_handle
    }

    /// Number of pointer presses recorded on the proof surface so far.
    #[must_use]
    pub fn clicks(&self) -> usize {
        self.clicks
    }

    /// Records one pointer press to prove event dispatch reaches entities.
    fn handle_press(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.clicks += 1;
        cx.notify();
    }

    fn collect_conversation_effects(
        &mut self,
        host: Entity<conversation_host::ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        // Host and controller queues are bounded by the same public maximum;
        // this explicit bound keeps the controller-flush retry finite.
        for _ in 0..=PROOF_MAX_CONVERSATION_EFFECTS {
            let (pending, total_pending) = {
                let host = host.read(cx);
                (
                    host.pending_effect_count(),
                    host.total_pending_effect_count(),
                )
            };
            let available =
                PROOF_MAX_CONVERSATION_EFFECTS.saturating_sub(self.conversation_effects.len());
            if pending > available || total_pending == 0 {
                break;
            }
            let effects = host.update(cx, |host, _| host.drain_effects());
            if effects.is_empty() {
                if pending > 0 {
                    break;
                }
                continue;
            }
            self.conversation_effects.extend(effects);
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Performs one explicit proof-to-application backpressure handoff.
    fn pump_conversation_boundary(
        &mut self,
        host: Entity<conversation_host::ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        self.collect_conversation_effects(host.clone(), cx);
        if host.read(cx).total_pending_effect_count() == 0 {
            host.update(cx, |host, host_cx| host.process_pending_actions(host_cx));
        }
        self.collect_conversation_effects(host, cx);
    }
}

/// Keeps the existing picker-only proof traversal contract while the genuine
/// conversation remains visible beside it. The conversation surface retains
/// its normal focus handles for direct application focus; only this
/// feasibility window removes those two handles from its shared Tab map.
fn suppress_conversation_tab_stops(
    host: &Entity<conversation_host::ConversationHost>,
    cx: &Context<ProofSurface>,
) {
    let (transcript_focus, disclosure_focus) = {
        let host = host.read(cx);
        let surface = host.surface().read(cx);
        (
            surface.transcript_focus_handle().clone(),
            surface.disclosure_focus_handle().clone(),
        )
    };
    transcript_focus.tab_stop(false);
    disclosure_focus.tab_stop(false);
}

impl Render for ProofSurface {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .key_context(PROOF_KEY_CONTEXT)
            .on_action(|_: &NextTabStop, window, _| window.focus_next())
            .on_action(|_: &PreviousTabStop, window, _| window.focus_prev())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_press))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .size_full()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(FOREGROUND))
            .text_xl()
            .child(format!("Artisan GPUI proof — clicks: {}", self.clicks))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Feasibility-only presentation · quit with cmd-q / ctrl-q"),
            )
            // The first real native workflow leaf: the project picker.
            .child(self.picker.clone())
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(picker_summary(self.picker.read(cx).last_action())),
            )
            .child({
                let mut conversation_panel = div()
                    .w_full()
                    .h(px(CONVERSATION_HEIGHT))
                    .rounded(px(8.0))
                    .bg(rgb(BACKGROUND));
                if let Some(host) = self.conversation_host.clone() {
                    conversation_panel = conversation_panel.child(host);
                } else {
                    conversation_panel = conversation_panel
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("conversation host unavailable");
                }
                conversation_panel
            })
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "conversation effects queued: {}",
                        self.conversation_effects.len()
                    )),
            )
    }
}

/// Proof catalog for the picker leaf: three demo attachments with the first
/// one current, so the leaf's current-row focus behavior is observable.
fn proof_catalog() -> (
    Vec<project_picker::ProjectOption>,
    Option<artisan_domain::ProjectId>,
) {
    use artisan_domain::ProjectId;

    let mut projects = Vec::with_capacity(3);
    for name in ["core", "docs-site", "playground"] {
        let Ok(id) = ProjectId::parse(format!("proof-{name}")) else {
            return (Vec::new(), None);
        };
        projects.push(project_picker::ProjectOption {
            id,
            name: name.to_owned().into(),
        });
    }
    let current = projects.first().map(|project| project.id.clone());
    (projects, current)
}

/// One-line observation of what the picker leaf has done so far.
fn picker_summary(action: Option<project_picker::ProjectPickerAction>) -> String {
    match action {
        Some(chosen) => format!("picker: {chosen:?}"),
        None => "picker: no action yet".to_string(),
    }
}

/// Launches the feasibility window and maps startup success onto the exit
/// code.
#[must_use]
pub fn run() -> ExitCode {
    let launched = Rc::new(Cell::new(false));
    let launch_flag = Rc::clone(&launched);

    Application::new().run(move |cx: &mut App| {
        bind_proof_actions(cx);
        // Fallback dispatcher when no focused element handles `Quit`.
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(SURFACE_WIDTH), px(SURFACE_HEIGHT)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(WINDOW_TITLE.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| ProofSurface::new(window, cx)),
        );

        match opened {
            Ok(_) => {
                launch_flag.set(true);
                cx.activate(true);
            }
            Err(error) => {
                eprintln!("phase-1 GPUI proof could not open its window: {error:?}");
                cx.quit();
            }
        }
    });

    if launched.get() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
