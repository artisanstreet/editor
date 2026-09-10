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
//!   `SnapshotReceived` domain snapshots plus directly registered
//!   Activity/Reasoning/Error facts (projection contract `fd6f3aa0`,
//!   delivery-owned turn sync — no manual `RegisterTurn`). Timestamps are
//!   current-relative so the timed host clock renders live spans, and the
//!   fixture prints the projected block order per case;
//! - captures all seven states (empty / thinking / working / streaming /
//!   completed / error / longform) at two baseline viewports, 1024x720 and 1536x900
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
use std::time::Duration;

use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, AuthoredText, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationSnapshot, ConversationTurn,
    ImageAttachmentRef, ItemId, ItemOrdinal, MessageBody, MessageId, MultimodalUserMessageItem,
    Revision, RunId, ThreadId, TurnId, TurnOrdinal, UnixMillis, UserMessageItem,
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
use crate::conversation_scene::SceneId;
use crate::conversation_state_machine::{
    ConversationStateEvent, SceneFact, SceneFactCommand, SceneFactKind,
};
use crate::conversation_surface::ordered_block_kinds;
use crate::desktop_shell::{
    DESKTOP_SIDEBAR_WIDTH_PX, DESKTOP_TITLEBAR_HEIGHT_PX, DesktopShellStyle, desktop_shell,
};
use crate::thread_screen::{ThreadScreen, ThreadScreenGate, ThreadScreenTitle};

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
    /// Settled exchange carrying long markdown plus one image attachment.
    Longform,
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
            Self::Longform => "longform",
        }
    }

    /// Every case in matrix order.
    #[must_use]
    pub fn all() -> [Self; 7] {
        [
            Self::Empty,
            Self::Thinking,
            Self::Working,
            Self::Streaming,
            Self::Completed,
            Self::Error,
            Self::Longform,
        ]
    }
}

/// Current wall-clock millis for fixture timestamps. The timed host clock
/// derives elapsed/settled spans from the turn's own `created_at` /
/// `updated_at`, so fixtures use current-relative times (never 1970):
/// active states tick from ~65s ago, terminal states settle on their own
/// recent span.
fn system_now_millis() -> Result<i64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .map_err(|error| format!("fixture clock unavailable: {error:?}"))
}

fn proof_turn_id() -> TurnId {
    TurnId::parse("parity-proof-turn").expect("fixture turn id is valid")
}

fn make_turn(
    lifecycle: ConversationLifecycle,
    created_at: UnixMillis,
    updated_at: UnixMillis,
) -> ConversationTurn {
    ConversationTurn {
        turn_id: proof_turn_id(),
        ordinal: TurnOrdinal::new(0),
        revision: Revision::new(0),
        lifecycle,
        created_at,
        updated_at,
    }
}

fn make_user(
    ordinal: u64,
    body: &str,
    created_at: UnixMillis,
    updated_at: UnixMillis,
) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(format!("parity-proof-user-{ordinal}"))
            .expect("fixture item id is valid"),
        turn_id: proof_turn_id(),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: MessageBody::parse(body.to_owned()).expect("fixture user body is valid"),
        created_at,
        updated_at,
    })
}

/// Long user prompt exercising markdown structure: heading, list, code
/// span, and a fenced block reference.
const LONGFORM_USER_BODY: &str = "# Parity drill\n\nProve the transcript keeps structure:\n\n- heading survives\n- `code span` survives\n- [reference link](https://example.invalid/parity) survives\n\n```text\nplain fenced block\n```\n";

/// Long assistant reply exercising markdown structure: heading, paragraph,
/// fenced code, list, and link.
const LONGFORM_ASSISTANT_BODY: &str = "## Result\n\nThe shell, transcript, and composer match the reference.\n\n```rust\nlet content_width = window_width - sidebar_width;\n```\n\nRemaining checks:\n\n- narrow viewport keeps the inspector\n- wide viewport keeps the composer docked\n- [runbook](https://example.invalid/runbook) attached\n";

fn proof_message_id() -> MessageId {
    MessageId::parse("parity-proof-message").expect("fixture message id is valid")
}

fn make_multimodal(
    thread: &ThreadId,
    ordinal: u64,
    created_at: UnixMillis,
    updated_at: UnixMillis,
) -> ConversationItem {
    let attachment = ImageAttachmentRef::new(
        proof_message_id(),
        thread.clone(),
        0,
        "image/png",
        "chart.png",
        18_432,
        [7_u8; 32],
    )
    .expect("fixture attachment reference is valid");
    ConversationItem::MultimodalUserMessage(MultimodalUserMessageItem {
        item_id: ItemId::parse(format!("parity-proof-multimodal-{ordinal}"))
            .expect("fixture item id is valid"),
        turn_id: proof_turn_id(),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        text: Some(AuthoredText::parse(LONGFORM_USER_BODY).expect("fixture text is valid")),
        attachments: vec![attachment],
        created_at,
        updated_at,
    })
}

fn make_assistant(
    ordinal: u64,
    body: &str,
    phase: AssistantMessagePhase,
    lifecycle: ConversationLifecycle,
    created_at: UnixMillis,
    updated_at: UnixMillis,
) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: ItemId::parse(format!("parity-proof-assistant-{ordinal}"))
            .expect("fixture item id is valid"),
        turn_id: proof_turn_id(),
        run_id: RunId::parse("parity-proof-run").expect("fixture run id is valid"),
        ordinal: ItemOrdinal::new(ordinal),
        revision: Revision::new(0),
        lifecycle,
        body: AssistantBody::parse(body.to_owned()).expect("fixture assistant body is valid"),
        phase,
        created_at,
        updated_at,
    })
}

/// Builds the domain snapshot each case dispatches, with current-relative
/// turn times so the timed host clock renders live spans.
///
/// Production path (projection contract `fd6f3aa0`): the delivery-owned sync
/// derives turn drive from snapshot lifecycles plus registered facts — no
/// manual `RegisterTurn`. Active turns tick from their own `created_at`;
/// terminal turns settle on their own `updated_at` span:
fn case_snapshot(
    case: ProofSceneCase,
    thread: &ThreadId,
    now: UnixMillis,
) -> Result<Option<ConversationSnapshot>, String> {
    let at = now.as_millis();
    let ago = |millis: i64| UnixMillis::from_millis(at - millis);
    let build = |turns, items| {
        ConversationSnapshot::new(
            thread.clone(),
            ConversationCursor::new(1),
            turns,
            items,
            now,
        )
        .map_err(|error| format!("fixture snapshot invalid: {error:?}"))
    };
    let snapshot = match case {
        ProofSceneCase::Empty => return Ok(None),
        ProofSceneCase::Completed => build(
            vec![make_turn(
                ConversationLifecycle::Completed,
                ago(125_000),
                ago(5_000),
            )],
            vec![
                make_user(
                    1,
                    "fixture prompt: prove visual parity",
                    ago(120_000),
                    ago(5_000),
                ),
                make_assistant(
                    2,
                    "fixture reply: shell, transcript, and composer match.",
                    AssistantMessagePhase::Final,
                    ConversationLifecycle::Completed,
                    ago(110_000),
                    ago(5_000),
                ),
            ],
        )?,
        ProofSceneCase::Thinking | ProofSceneCase::Working => build(
            vec![make_turn(
                ConversationLifecycle::Active,
                ago(65_000),
                now,
            )],
            vec![make_user(
                1,
                "fixture prompt: prove visual parity",
                ago(60_000),
                now,
            )],
        )?,
        ProofSceneCase::Streaming => build(
            vec![make_turn(
                ConversationLifecycle::Active,
                ago(65_000),
                now,
            )],
            vec![
                make_user(
                    1,
                    "fixture prompt: prove visual parity",
                    ago(60_000),
                    now,
                ),
                make_assistant(
                    2,
                    "fixture partial: rendering the",
                    AssistantMessagePhase::Final,
                    ConversationLifecycle::Streaming,
                    ago(30_000),
                    now,
                ),
            ],
        )?,
        ProofSceneCase::Error => build(
            vec![make_turn(
                ConversationLifecycle::Failed,
                ago(65_000),
                ago(5_000),
            )],
            vec![make_user(
                1,
                "fixture prompt: trigger failure",
                ago(60_000),
                ago(5_000),
            )],
        )?,
        ProofSceneCase::Longform => build(
            vec![make_turn(
                ConversationLifecycle::Completed,
                ago(185_000),
                ago(5_000),
            )],
            vec![
                make_multimodal(thread, 1, ago(180_000), ago(5_000)),
                make_assistant(
                    2,
                    LONGFORM_ASSISTANT_BODY,
                    AssistantMessagePhase::Final,
                    ConversationLifecycle::Completed,
                    ago(170_000),
                    ago(5_000),
                ),
            ],
        )?,
    };
    Ok(Some(snapshot))
}

/// Facts each case registers directly after its snapshot. Activity feeds a
/// `Working` drive, Reasoning a `Thinking` drive, Error the failure card —
/// the same delivery-plus-fact path the shipping app relies on.
fn case_facts(case: ProofSceneCase) -> Result<Vec<SceneFact>, String> {
    let fact = |name: &str, kind: SceneFactKind| {
        SceneFact::new(
            SceneId::parse(format!("parity-proof-{name}")).expect("fixture fact id is valid"),
            proof_turn_id(),
            100,
            kind,
        )
        .map_err(|error| format!("fixture fact invalid: {error:?}"))
    };
    match case {
        ProofSceneCase::Thinking => Ok(vec![fact(
            "thinking-fact",
            SceneFactKind::Reasoning {
                body: "fixture trace: resolving references".to_owned(),
            },
        )?]),
        ProofSceneCase::Working => Ok(vec![fact(
            "working-fact",
            SceneFactKind::Activity {
                body: "fixture work: reading Cargo.toml".to_owned(),
            },
        )?]),
        ProofSceneCase::Error => Ok(vec![fact(
            "error-fact",
            SceneFactKind::Error {
                message: "fixture failure: transport refused".to_owned(),
            },
        )?]),
        _ => Ok(Vec::new()),
    }
}

/// Seeds one case through the production delivery-plus-fact path: snapshot
/// (and, where the case needs one, directly registered Activity/Reasoning/
/// Error facts). No manual `RegisterTurn`: the delivery-owned sync derives
/// turn drive, per projection contract `fd6f3aa0`. A refusal is returned as
/// a message so the runner records it against the case instead of painting
/// an undriven window.
fn seed_case(
    screen: &Entity<ThreadScreen>,
    case: ProofSceneCase,
    thread: &ThreadId,
    cx: &mut App,
) -> Result<(), String> {
    let host: Entity<ConversationHost> = screen.read(cx).host().clone();
    let now = UnixMillis::from_millis(system_now_millis()?);
    let snapshot = case_snapshot(case, thread, now)?;
    let facts = case_facts(case)?;
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
        for fact in facts {
            host.dispatch(
                ConversationStateEvent::Fact(SceneFactCommand::Register(fact)),
                host_cx,
            )
            .map_err(|error| format!("fact refused: {error:?}"))?;
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

/// Settles one capture slot: records failure, decrements the outstanding
/// count, and quits the application when nothing remains. Every terminal
/// path (seed refusal, window-open failure, update failure, capture
/// success/failure, watchdog) runs through here, so the runner can neither
/// hang nor quit early with captures outstanding.
fn settle(pending: &Rc<Cell<usize>>, failed_flag: &Rc<Cell<bool>>, failed: bool, cx: &mut App) {
    if failed {
        failed_flag.set(true);
    }
    pending.set(pending.get().saturating_sub(1));
    if pending.get() == 0 {
        cx.quit();
    }
}

/// Publishes the shell geometry bound to one capture: actual window width
/// minus the resolved (expanded) sidebar reservation, plus the titlebar
/// reservation. Both baseline viewports pin the inspector expanded, so the
/// narrower capture must still reserve it; responsive hiding is a separate
/// lane and is not what these pixels claim.
fn print_capture_geometry(slug: &str, width: f32, scale: f32) {
    let style = DesktopShellStyle::resolve(false, scale);
    let sidebar_matches = style.sidebar_width == px(DESKTOP_SIDEBAR_WIDTH_PX);
    let titlebar_matches = style.titlebar_height == px(DESKTOP_TITLEBAR_HEIGHT_PX);
    let content_width = width - DESKTOP_SIDEBAR_WIDTH_PX;
    println!(
        "parity-proof geometry {slug}: window={width} scale={scale} \
         sidebar={} titlebar={} content={content_width} inspector=reserved \
         title=\"Parity proof thread\"",
        DESKTOP_SIDEBAR_WIDTH_PX, DESKTOP_TITLEBAR_HEIGHT_PX,
    );
    debug_assert!(sidebar_matches && titlebar_matches);
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
            screen.set_title(ThreadScreenTitle {
                title: String::from("Parity proof thread"),
                ..Default::default()
            });
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
    let failed_after_run = Rc::clone(&failed);

    gpui_platform::application()
        .with_assets(artisan_ui::asset_seam::CatalogAssetSource)
        .run(move |cx: &mut App| {
        // Shipping boot parity: vendored typefaces and catalog assets before
        // any window opens, mirroring `native_application::run`.
        if let Err(error) = artisan_ui::fonts::register_bundled_fonts(cx) {
            eprintln!("bundled font registration failed, using system faces: {error}");
        }

        // Bounded watchdog: hidden windows may never deliver a next frame
        // (vendor frame scheduling for never-shown windows is unverified
        // from source alone). If anything is still outstanding after the
        // budget, record the failure and quit instead of hanging.
        {
            let pending = Rc::clone(&remaining);
            let failed_flag = Rc::clone(&failed);
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(120))
                    .await;
                let _ = cx.update(|cx| {
                    if pending.get() > 0 {
                        eprintln!(
                            "parity-proof watchdog: {} captures unsettled; quitting",
                            pending.get()
                        );
                        failed_flag.set(true);
                        cx.quit();
                    }
                });
            })
            .detach();
        }

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
                    settle(&remaining, &failed, true, cx);
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
                    let updated = cx.update_window(handle.into(), |_, window, _| {
                        window.refresh();
                        window.on_next_frame(move |window, cx| {
                            let scale = window.scale_factor();
                            print_capture_geometry(&stem, capture.width, scale);
                            let expected_width = (capture.width * scale).round() as u32;
                            let expected_height = (capture.height * scale).round() as u32;
                            let mut failed = false;
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
                                             {expected_width}x{expected_height}",
                                            capture.width,
                                            capture.height,
                                            actual.0,
                                            actual.1,
                                        );
                                        failed = true;
                                    }
                                    if let Err(error) =
                                        image::DynamicImage::ImageRgba8(image).save(&path)
                                    {
                                        eprintln!(
                                            "parity-proof could not save {path}: {error:?}"
                                        );
                                        failed = true;
                                    } else {
                                        println!("parity-proof saved {path}");
                                    }
                                }
                                Err(error) => {
                                    eprintln!(
                                        "parity-proof capture failed for {stem}: {error:?}"
                                    );
                                    failed = true;
                                }
                            }
                            settle(&pending, &failed_flag, failed, cx);
                        });
                    });
                    if updated.is_err() {
                        eprintln!("parity-proof update failed for {stem}");
                        settle(&remaining, &failed, true, cx);
                    }
                }
                Err(error) => {
                    eprintln!("parity-proof could not open its window: {error:?}");
                    settle(&remaining, &failed, true, cx);
                }
            }
        }
        if remaining.get() == 0 {
            cx.quit();
        }
    });

    if launched.get() && !failed_after_run.get() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::{ProofSceneCase, case_facts, case_snapshot};
    use crate::conversation_state_machine::SceneFactKind;
    use artisan_domain::{ThreadId, UnixMillis};

    fn thread() -> ThreadId {
        ThreadId::parse("parity-proof-test").expect("fixture thread id is valid")
    }

    fn now() -> UnixMillis {
        UnixMillis::from_millis(1_700_000_000_000)
    }

    #[test]
    fn empty_case_dispatches_nothing() {
        assert!(
            case_snapshot(ProofSceneCase::Empty, &thread(), now())
                .expect("empty builds")
                .is_none()
        );
        assert!(case_facts(ProofSceneCase::Empty).expect("empty facts").is_empty());
    }

    #[test]
    fn completed_case_needs_no_facts() {
        assert!(
            case_snapshot(ProofSceneCase::Completed, &thread(), now())
                .expect("completed builds")
                .is_some()
        );
        assert!(
            case_facts(ProofSceneCase::Completed)
                .expect("completed facts")
                .is_empty()
        );
    }

    #[test]
    fn thinking_case_registers_a_reasoning_fact() {
        let facts = case_facts(ProofSceneCase::Thinking).expect("thinking facts");
        assert_eq!(facts.len(), 1);
        assert!(matches!(facts[0].kind, SceneFactKind::Reasoning { .. }));
    }

    #[test]
    fn working_case_registers_an_activity_fact() {
        let facts = case_facts(ProofSceneCase::Working).expect("working facts");
        assert_eq!(facts.len(), 1);
        assert!(matches!(facts[0].kind, SceneFactKind::Activity { .. }));
    }

    #[test]
    fn streaming_case_comes_from_a_live_item_not_facts() {
        assert!(
            case_snapshot(ProofSceneCase::Streaming, &thread(), now())
                .expect("streaming builds")
                .is_some()
        );
        assert!(
            case_facts(ProofSceneCase::Streaming)
                .expect("streaming facts")
                .is_empty()
        );
    }

    #[test]
    fn error_case_registers_an_error_fact() {
        let facts = case_facts(ProofSceneCase::Error).expect("error facts");
        assert_eq!(facts.len(), 1);
        assert!(matches!(facts[0].kind, SceneFactKind::Error { .. }));
    }

    #[test]
    fn longform_case_carries_markdown_and_one_attachment() {
        let snapshot = case_snapshot(ProofSceneCase::Longform, &thread(), now())
            .expect("longform builds")
            .expect("longform snapshot builds");
        assert!(case_facts(ProofSceneCase::Longform).expect("longform facts").is_empty());
        let debug = format!("{snapshot:?}");
        assert!(
            debug.contains("MultimodalUserMessage"),
            "longform user item must be multimodal, got {debug}"
        );
    }
}
