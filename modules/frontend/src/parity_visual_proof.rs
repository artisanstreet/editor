//! Visual-parity proof fixture: production shell + thread screen scenes.
//!
//! This module is intentionally a **new leaf only**. It edits no product
//! surface, no manifest, and no Svelte reference. Registration (`mod
//! parity_visual_proof;`, plus any feature gate and binary/export wiring) is
//! owned by the root: until then this file is inert and uncompiled.
//!
//! What the fixture does, once registered and run on Windows:
//!
//! - mounts the production [`ThreadScreen`](crate::thread_screen::ThreadScreen)
//!   (host + composer) inside the production
//!   [`desktop_shell`](crate::desktop_shell::desktop_shell) wrapper, so the
//!   captured pixels exercise the real shell background, the real 48 px
//!   titlebar reservation, the real 218 px sidebar reservation, and the real
//!   composer dock — no hand-drawn approximation of any of them;
//! - seeds every synthetic state through the **real controller path**:
//!   [`ConversationHost::dispatch`](crate::conversation_host::ConversationHost::dispatch)
//!   with `SnapshotReceived` domain snapshots plus `RegisterTurn` / `Turn`
//!   events, copied from the `conversation_host` black-box tests. Nothing
//!   paints a hand-built scene: the surface renders whatever the controller
//!   projects, and the fixture prints the projected block order per case;
//! - captures all six states (empty / thinking / working / streaming /
//!   completed / error) at two baseline viewports, 1024x720 and 1536x900
//!   logical, hidden (`show: false`, never presented, no OS screen capture,
//!   no Win32 control).
//!
//! Capture assumes `Window::render_to_image` is enabled (root enables the
//! `test-support` feature; the shipping-wgpu readback itself is owned by the
//! separate capture lane). There is deliberately **no** DirectX-fallback
//! screenshot path: a non-wgpu capture proves nothing about the shipping
//! renderer.

#![forbid(unsafe_code)]

use std::cell::Cell;
use std::process::ExitCode;
use std::rc::Rc;

use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationSnapshot, ConversationTurn, ItemId,
    ItemOrdinal, MessageBody, Revision, RunId, ThreadId, TurnId, TurnOrdinal, UnixMillis,
    UserMessageItem,
};
use artisan_ui::theme::{DesktopTheme, ThemeMode};
use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, Window, WindowBounds,
    WindowOptions, div,
    prelude::{IntoElement, Render},
    px, size,
};

use crate::conversation_delivery_machine::ConversationDeliveryEvent;
use crate::conversation_host::{ConversationHost, ConversationHostError};
use crate::conversation_state_machine::ConversationStateEvent;
use crate::conversation_surface::ordered_block_kinds;
use crate::conversation_turn_machine::{FailureKind, TurnEvent};
use crate::desktop_shell::desktop_shell;
use crate::thread_screen::{ThreadScreen, ThreadScreenGate};

/// Narrow baseline viewport, logical pixels.
const NARROW_LOGICAL_WIDTH: f32 = 1024.0;
/// Narrow baseline viewport height, logical pixels.
const NARROW_LOGICAL_HEIGHT: f32 = 720.0;
/// Wide baseline viewport, logical pixels.
const WIDE_LOGICAL_WIDTH: f32 = 1536.0;
/// Wide baseline viewport height, logical pixels.
const WIDE_LOGICAL_HEIGHT: f32 = 900.0;

/// Reference-screenshot display scale (125%). Recorded, not assumed: each
/// capture names the window's measured scale factor and asserts the saved
/// image dimensions against `logical * measured scale`.
const REFERENCE_SCALE: f32 = 1.25;

/// Synthetic conversation states, every one seeded through the controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProofSceneCase {
    /// Genuinely empty conversation: no dispatch at all.
    Empty,
    /// Active turn with a `Thinking` event.
    Thinking,
    /// Active turn with a `Working` event.
    Working,
    /// Active turn through `Thinking` into `StreamingReply`.
    Streaming,
    /// Settled user + final-assistant exchange on a completed turn.
    Completed,
    /// Active turn with a `Failed` event.
    Error,
}

impl ProofSceneCase {
    /// Stable case slug used in thread ids and file names.
    #[must_use]
    pub fn slug(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Thinking => "thinking",
            Self::Working => "working",
            Self::Streaming => "streaming",
            Self::Completed => "completed",
            Self::Error => "error",
        }
    }

    /// Every case in matrix order.
    #[must_use]
    pub fn all() -> [Self; 6] {
        [
            Self::Empty,
            Self::Thinking,
            Self::Working,
            Self::Streaming,
            Self::Completed,
            Self::Error,
        ]
    }
}

fn stamp(millis: i64) -> UnixMillis {
    UnixMillis::from_millis(millis)
}

fn proof_turn_id() -> TurnId {
    TurnId::parse("parity-proof-turn").expect("fixture turn id is valid")
}

fn make_turn(lifecycle: ConversationLifecycle) -> ConversationTurn {
    ConversationTurn {
        turn_id: proof_turn_id(),
        ordinal: TurnOrdinal::new(0),
        revision: Revision::new(0),
        lifecycle,
        created_at: stamp(0),
        updated_at: stamp(10),
    }
}

fn make_user(ordinal: u64, body: &str) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(format!("parity-proof-user-{ordinal}"))
            .expect("fixture item id is valid"),
        turn_id: proof_turn_id(),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: MessageBody::parse(body.to_owned()).expect("fixture user body is valid"),
        created_at: stamp(1),
        updated_at: stamp(10),
    })
}

fn make_assistant(
    ordinal: u64,
    body: &str,
    phase: AssistantMessagePhase,
) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: ItemId::parse(format!("parity-proof-assistant-{ordinal}"))
            .expect("fixture item id is valid"),
        turn_id: proof_turn_id(),
        run_id: RunId::parse("parity-proof-run").expect("fixture run id is valid"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: AssistantBody::parse(body.to_owned()).expect("fixture assistant body is valid"),
        phase,
        created_at: stamp(2),
        updated_at: stamp(10),
    })
}

/// Builds the domain snapshot each case dispatches. The empty case has no
/// snapshot: its state is the fresh mount. Active cases carry an `Active`
/// turn plus the user prompt; the completed case mirrors the black-box
/// baseline (completed turn, user + final assistant).
fn case_snapshot(case: ProofSceneCase, thread: &ThreadId) -> Option<ConversationSnapshot> {
    let snapshot = match case {
        ProofSceneCase::Empty => return None,
        ProofSceneCase::Completed => ConversationSnapshot::new(
            thread.clone(),
            ConversationCursor::new(1),
            vec![make_turn(ConversationLifecycle::Completed)],
            vec![
                make_user(1, "fixture prompt: prove visual parity"),
                make_assistant(
                    2,
                    "fixture reply: shell, transcript, and composer match.",
                    AssistantMessagePhase::Final,
                ),
            ],
            stamp(10),
        ),
        ProofSceneCase::Thinking | ProofSceneCase::Working | ProofSceneCase::Streaming => {
            ConversationSnapshot::new(
                thread.clone(),
                ConversationCursor::new(1),
                vec![make_turn(ConversationLifecycle::Active)],
                vec![make_user(1, "fixture prompt: prove visual parity")],
                stamp(10),
            )
        }
        ProofSceneCase::Error => ConversationSnapshot::new(
            thread.clone(),
            ConversationCursor::new(1),
            vec![make_turn(ConversationLifecycle::Active)],
            vec![make_user(1, "fixture prompt: trigger failure")],
            stamp(10),
        ),
    };
    Some(snapshot.expect("fixture snapshot is valid"))
}

/// Turn events after `RegisterTurn` for the active cases. The streaming
/// sequence mirrors the `streaming_narration` black-box test exactly.
fn case_turn_events(case: ProofSceneCase) -> Vec<TurnEvent> {
    match case {
        ProofSceneCase::Empty | ProofSceneCase::Completed => Vec::new(),
        ProofSceneCase::Thinking => vec![TurnEvent::Thinking { at: 1, revision: 1 }],
        ProofSceneCase::Working => vec![TurnEvent::Working { at: 1, revision: 1 }],
        ProofSceneCase::Streaming => vec![
            TurnEvent::Thinking { at: 1, revision: 1 },
            TurnEvent::StreamingReply { at: 2, revision: 2 },
        ],
        ProofSceneCase::Error => vec![TurnEvent::Failed {
            at: 1,
            revision: 1,
            kind: Some(FailureKind::Generic),
        }],
    }
}

/// Seeds one case through the public host boundary. A refusal is returned
/// as a message so the runner records it against the case instead of
/// painting an undriven window.
fn seed_case(
    screen: &Entity<ThreadScreen>,
    case: ProofSceneCase,
    thread: &ThreadId,
    cx: &mut App,
) -> Result<(), String> {
    let host: Entity<ConversationHost> = screen.read(cx).host().clone();
    let snapshot = case_snapshot(case, thread);
    let events = case_turn_events(case);
    let turn_id = proof_turn_id();
    host.update(cx, |host, host_cx| {
        if let Some(snapshot) = snapshot {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::SnapshotReceived(
                    snapshot,
                )),
                host_cx,
            )
            .map_err(|error| format!("snapshot refused: {error:?}"))?;
        }
        if events.is_empty() {
            return Ok(());
        }
        host.dispatch(
            ConversationStateEvent::RegisterTurn {
                turn_id: turn_id.clone(),
            },
            host_cx,
        )
        .map_err(|error| format!("register refused: {error:?}"))?;
        for event in events {
            host.dispatch(
                ConversationStateEvent::Turn {
                    turn_id: turn_id.clone(),
                    event,
                },
                host_cx,
            )
            .map_err(|error| format!("turn event refused: {error:?}"))?;
        }
        Ok(())
    })
}

/// Prints the controller-projected block order for one seeded case: the
/// surface renders exactly this scene, so the manifest binds the saved
/// pixels to real projection output rather than a hand-built expectation.
fn print_case_manifest(screen: &Entity<ThreadScreen>, case: ProofSceneCase, cx: &mut App) {
    let host = screen.read(cx).host().clone();
    match host.read(cx).controller_scene() {
        Ok(scene) => println!(
            "parity-proof case={} blocks={:?}",
            case.slug(),
            ordered_block_kinds(&scene)
        ),
        Err(error) => println!(
            "parity-proof case={} projection unavailable: {error:?}",
            case.slug()
        ),
    }
}

/// Fixture root: production thread screen inside the production desktop
/// shell.
///
/// The identity/search/sidebar slots are fixture-owned empty elements with
/// no reference pixels behind them; they exist only because the wrapper
/// signature requires them. The validated geometry (titlebar height, sidebar
/// width reservation, shell background continuity) and every transcript and
/// composer pixel come from production code driven through the controller.
pub struct ParityProofShell {
    screen: Entity<ThreadScreen>,
}

impl ParityProofShell {
    /// Mounts the production thread screen, seeds one case, and opens its
    /// gate, mirroring the shipping route after its thread-open snapshot.
    ///
    /// # Errors
    ///
    /// Returns the mount refusal, or the first controller refusal while
    /// seeding, as a message bound to the case.
    pub fn mount(
        thread_id: ThreadId,
        case: ProofSceneCase,
        cx: &mut App,
    ) -> Result<Entity<Self>, String> {
        let screen = ThreadScreen::mount(thread_id.clone(), ThemeMode::Dark, cx)
            .map_err(|error| format!("mount refused: {error:?}"))?;
        seed_case(&screen, case, &thread_id, cx)?;
        screen.update(cx, |screen, _| {
            screen.set_gate(ThreadScreenGate::Open);
        });
        print_case_manifest(&screen, case, cx);
        Ok(cx.new(|_| Self { screen }))
    }

    /// Renders the shell around the mounted screen.
    fn render_shell(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> AnyElement {
        let body = self.screen.clone().into_any_element();
        desktop_shell(
            DesktopTheme::neutral_dark(),
            false,
            div().into_any_element(),
            div().into_any_element(),
            div().into_any_element(),
            body,
            window.scale_factor(),
            window.is_maximized(),
        )
        .into_any_element()
    }
}

impl Render for ParityProofShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_shell(window, cx)
    }
}

/// One capture target: case plus viewport.
struct ProofCapture {
    case: ProofSceneCase,
    viewport_slug: &'static str,
    width: f32,
    height: f32,
}

impl ProofCapture {
    /// File stem encoding case and exact logical viewport.
    fn file_stem(&self) -> String {
        format!(
            "parity-proof-{}-{}-{}x{}",
            self.case.slug(),
            self.viewport_slug,
            self.width as u32,
            self.height as u32
        )
    }
}

/// Opens one hidden window per case and viewport, captures each on its next
/// frame through the production shell pixels, saves the PNGs, and maps
/// success onto the exit code.
///
/// Requires `Window::render_to_image` (root-owned `test-support`
/// enablement) and the capture lane's shipping-wgpu readback.
#[must_use]
pub fn run() -> ExitCode {
    let launched = Rc::new(Cell::new(false));
    let launch_flag = Rc::clone(&launched);
    let captures: Vec<ProofCapture> = ProofSceneCase::all()
        .into_iter()
        .flat_map(|case| {
            [
                ProofCapture {
                    case,
                    viewport_slug: "narrow",
                    width: NARROW_LOGICAL_WIDTH,
                    height: NARROW_LOGICAL_HEIGHT,
                },
                ProofCapture {
                    case,
                    viewport_slug: "wide",
                    width: WIDE_LOGICAL_WIDTH,
                    height: WIDE_LOGICAL_HEIGHT,
                },
            ]
        })
        .collect();
    let remaining = Rc::new(Cell::new(captures.len()));
    let failed = Rc::new(Cell::new(false));

    gpui_platform::application().run(move |cx: &mut App| {
        for capture in captures {
            let stem = capture.file_stem();
            let thread_id = ThreadId::parse(format!(
                "parity-proof-{}-{}",
                capture.case.slug(),
                capture.viewport_slug
            ))
            .expect("fixture thread id is valid");
            let shell = match ParityProofShell::mount(thread_id, capture.case, cx) {
                Ok(shell) => shell,
                Err(error) => {
                    eprintln!("parity-proof seed failed for {stem}: {error}");
                    failed.set(true);
                    continue;
                }
            };
            let bounds = Bounds::centered(
                None,
                size(px(capture.width), px(capture.height)),
                cx,
            );
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                move |_, _| shell,
            );
            match opened {
                Ok(handle) => {
                    launch_flag.set(true);
                    let pending = Rc::clone(&remaining);
                    let failed_flag = Rc::clone(&failed);
                    let _ = cx.update_window(handle.into(), |_, window, _| {
                        window.refresh();
                        window.on_next_frame(move |window, cx| {
                            let scale = window.scale_factor();
                            let expected_width = (capture.width * scale).round() as u32;
                            let expected_height = (capture.height * scale).round() as u32;
                            match window.render_to_image() {
                                Ok(image) => {
                                    let actual = (image.width(), image.height());
                                    let path = format!(
                                        "{stem}-scale{scale}-{}x{}.png",
                                        actual.0, actual.1
                                    );
                                    if actual != (expected_width, expected_height) {
                                        eprintln!(
                                            "parity-proof dimension mismatch for {stem}: \
                                             logical {}x{} at scale {scale} (reference \
                                             {REFERENCE_SCALE}) produced {}x{}, expected \
                                             {expected_width}x{expected_height}"
                                        );
                                        failed_flag.set(true);
                                    }
                                    if let Err(error) =
                                        image::DynamicImage::ImageRgba8(image).save(&path)
                                    {
                                        eprintln!(
                                            "parity-proof could not save {path}: {error:?}"
                                        );
                                        failed_flag.set(true);
                                    } else {
                                        println!("parity-proof saved {path}");
                                    }
                                }
                                Err(error) => {
                                    eprintln!(
                                        "parity-proof capture failed for {stem}: {error:?}"
                                    );
                                    failed_flag.set(true);
                                }
                            }
                            pending.set(pending.get().saturating_sub(1));
                            if pending.get() == 0 {
                                cx.quit();
                            }
                        });
                    });
                }
                Err(error) => {
                    eprintln!("parity-proof could not open its window: {error:?}");
                    failed.set(true);
                }
            }
        }
    });

    if launched.get() && !failed.get() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::{ProofSceneCase, case_snapshot, case_turn_events};
    use artisan_domain::ThreadId;

    fn thread() -> ThreadId {
        ThreadId::parse("parity-proof-test").expect("fixture thread id is valid")
    }

    #[test]
    fn empty_case_dispatches_nothing() {
        assert!(case_snapshot(ProofSceneCase::Empty, &thread()).is_none());
        assert!(case_turn_events(ProofSceneCase::Empty).is_empty());
    }

    #[test]
    fn completed_case_needs_no_turn_events() {
        assert!(case_snapshot(ProofSceneCase::Completed, &thread()).is_some());
        assert!(case_turn_events(ProofSceneCase::Completed).is_empty());
    }

    #[test]
    fn streaming_case_replays_thinking_then_reply() {
        use crate::conversation_turn_machine::TurnEvent;
        let events = case_turn_events(ProofSceneCase::Streaming);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], TurnEvent::Thinking { .. }));
        assert!(matches!(events[1], TurnEvent::StreamingReply { .. }));
    }

    #[test]
    fn every_active_case_constructs_a_snapshot() {
        for case in [
            ProofSceneCase::Thinking,
            ProofSceneCase::Working,
            ProofSceneCase::Streaming,
            ProofSceneCase::Error,
        ] {
            assert!(
                case_snapshot(case, &thread()).is_some(),
                "case {} must build a snapshot",
                case.slug()
            );
            assert!(
                !case_turn_events(case).is_empty(),
                "case {} must drive turn events",
                case.slug()
            );
        }
    }
}
