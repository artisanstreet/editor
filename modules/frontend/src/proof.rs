//! Phase 1 native-host feasibility proof window.
//!
//! FEASIBILITY ONLY: every value below (window size, colors, copy, click
//! counter) exists solely to prove that upstream GPUI compiles, links, and
//! runs against the real Windows platform stack — DirectWrite/DirectX
//! initialization, the `windows` import libraries, `ctor`/`inventory`
//! section registration, and the `actions!` macro expansion included. This is
//! not Artisan visual or interaction design; those follow UI archaeology.

use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Application, Bounds, Context, FocusHandle, KeyBinding, MouseButton,
    MouseDownEvent, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    prelude::{InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _},
    px, rgb, size,
};

// Feasibility-only quit action exercising the `actions!` macro registry.
actions!(proof, [Quit]);

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
struct ProofSurface {
    focus_handle: FocusHandle,
    clicks: usize,
}

impl Render for ProofSurface {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
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
    }
}

impl ProofSurface {
    /// Records one pointer press to prove event dispatch reaches entities.
    fn handle_press(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.clicks += 1;
        cx.notify();
    }
}

/// Launches the feasibility window and maps startup success onto the exit
/// code.
#[must_use]
pub fn run() -> ExitCode {
    let launched = Rc::new(Cell::new(false));
    let launch_flag = Rc::clone(&launched);

    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);
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
            |window, cx| {
                cx.new(|cx| {
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window);
                    ProofSurface {
                        focus_handle,
                        clicks: 0,
                    }
                })
            },
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
