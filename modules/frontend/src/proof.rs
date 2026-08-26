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

use crate::project_picker;
use gpui::{
    App, AppContext as _, Application, Bounds, Context, Entity, FocusHandle, KeyBinding,
    MouseButton, MouseDownEvent, TitlebarOptions, Window, WindowBounds, WindowOptions, actions,
    div,
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
const SURFACE_HEIGHT: f32 = 420.0;
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
            project_picker::ProjectPickerView::new(
                projects,
                current,
                artisan_ui::theme::ThemeMode::Dark,
                picker_cx,
            )
        });
        Self {
            focus_handle,
            clicks: 0,
            picker,
        }
    }

    /// The embedded project-picker leaf.
    #[must_use]
    pub fn picker(&self) -> &Entity<project_picker::ProjectPickerView> {
        &self.picker
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
    }
}

/// Proof catalog for the picker leaf: three demo attachments with the first
/// one current, so the leaf's current-row focus behavior is observable.
fn proof_catalog() -> (
    Vec<project_picker::ProjectOption>,
    Option<artisan_domain::ProjectId>,
) {
    use artisan_domain::ProjectId;

    let option = |name: &str| project_picker::ProjectOption {
        id: ProjectId::parse(format!("proof-{name}")).expect("proof ids are valid"),
        name: name.to_string().into(),
    };

    let projects = vec![option("core"), option("docs-site"), option("playground")];
    let current = ProjectId::parse("proof-core").expect("proof id is valid");
    (projects, Some(current))
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
