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
//! - mounts each case through the shell lane's proof factory
//!   (`ThreadScreen::mount_proof`: gate, content width, and title in one
//!   step), publishes the live content width (actual window bounds minus the
//!   resolved rail, notify-on-change) so narrow and wide viewports render
//!   different inspector states, and uses the shipping transparent-caption
//!   window setup;
//! - seeds every synthetic state through the **real controller path**:
//!   `SnapshotReceived` domain snapshots plus directly registered
//!   Activity/Reasoning/Error facts (projection contract `fd6f3aa0`,
//!   delivery-owned turn sync — no manual `RegisterTurn`). Timestamps are
//!   current-relative so the timed host clock renders live spans, and the
//!   fixture prints the projected block order per case;
//! - captures exactly one selected state at one baseline viewport per
//!   process (1024x720 or 1536x900 logical), hidden (`show: false`, never
//!   presented, no OS screen capture, no Win32 control). Selection is an
//!   explicit CLI pair and anything else fails closed. Nine cases
//!   (seven baseline plus two reference-complaint reproductions) run as
//!   sequential processes with GPU/RAM reclaimed between.
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
use artisan_ui::theme::DesktopTheme;
use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, TitlebarOptions,
    Window, WindowBounds, WindowOptions, div,
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
use crate::thread_screen::{ThreadScreen, thread_inspector_visible};

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
    /// Settled user complaint exchange with a run-attributed work session.
    ReferenceSettled,
    /// Live thinking summary with markdown fragments, run-attributed.
    ReferenceThinking,
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
            Self::ReferenceSettled => "reference-settled",
            Self::ReferenceThinking => "reference-thinking",
        }
    }

    /// Parses an exact case slug; anything else is `None` (fail closed).
    #[must_use]
    pub fn parse(slug: &str) -> Option<Self> {
        match slug {
            "empty" => Some(Self::Empty),
            "thinking" => Some(Self::Thinking),
            "working" => Some(Self::Working),
            "streaming" => Some(Self::Streaming),
            "completed" => Some(Self::Completed),
            "error" => Some(Self::Error),
            "longform" => Some(Self::Longform),
            "reference-settled" => Some(Self::ReferenceSettled),
            "reference-thinking" => Some(Self::ReferenceThinking),
            _ => None,
        }
    }

    /// Every case in matrix order (root runs each as its own process).
    #[must_use]
    pub fn all() -> [Self; 9] {
        [
            Self::Empty,
            Self::Thinking,
            Self::Working,
            Self::Streaming,
            Self::Completed,
            Self::Error,
            Self::Longform,
            Self::ReferenceSettled,
            Self::ReferenceThinking,
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

fn proof_run_id() -> RunId {
    RunId::parse("parity-proof-run").expect("fixture run id is valid")
}

/// Run identity shared by one reference exchange's assistant reply and its
/// session facts, so single-run session grouping engages exactly as the
/// reference single session does. Deterministic synthetic routing evidence
/// only, never a lease or credential.
fn reference_run_id() -> RunId {
    RunId::parse("parity-proof-reference").expect("fixture run id is valid")
}

fn make_assistant(
    ordinal: u64,
    body: &str,
    phase: AssistantMessagePhase,
    lifecycle: ConversationLifecycle,
    run_id: RunId,
    created_at: UnixMillis,
    updated_at: UnixMillis,
) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: ItemId::parse(format!("parity-proof-assistant-{ordinal}"))
            .expect("fixture item id is valid"),
        turn_id: proof_turn_id(),
        run_id,
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
                    proof_run_id(),
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
                    proof_run_id(),
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
                    proof_run_id(),
                    ago(170_000),
                    ago(5_000),
                ),
            ],
        )?,
        // User complaint reproduction: user "Whoopty", settled reply with
        // native emoji, run-attributed reasoning summary sharing the
        // reply's run so single-run session grouping engages from the real
        // pipeline (no producer emits WorkSession markers); the 6s
        // terminal span settles ThoughtFor{6000}.
        ProofSceneCase::ReferenceSettled => build(
            vec![make_turn(
                ConversationLifecycle::Completed,
                ago(6_000),
                now,
            )],
            vec![
                make_user(1, "Whoopty", ago(5_500), now),
                make_assistant(
                    2,
                    "Whoopty! \u{1F604} Whats up?",
                    AssistantMessagePhase::Final,
                    ConversationLifecycle::Completed,
                    reference_run_id(),
                    ago(5_000),
                    now,
                ),
            ],
        )?,
        // Live counterpart: active turn, same user prompt, markdown-rich
        // reasoning summary (inline code, strong, italic) attributed to the
        // run, so the live thinking summary line renders from provenance.
        ProofSceneCase::ReferenceThinking => build(
            vec![make_turn(
                ConversationLifecycle::Active,
                ago(65_000),
                now,
            )],
            vec![make_user(1, "Whoopty", ago(60_000), now)],
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
    // Run-attributed facts carry ordinals past the durable items and share
    // the reply's run, which is what single-run session grouping reads.
    // Attribution comes from retained observation identity, never text.
    let attributed_fact = |name: &str, ordinal: u64, kind: SceneFactKind| {
        SceneFact::new(
            SceneId::parse(format!("parity-proof-{name}")).expect("fixture fact id is valid"),
            proof_turn_id(),
            ordinal,
            kind,
        )
        .map(|fact| fact.with_run_id(reference_run_id()))
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
        ProofSceneCase::ReferenceSettled => Ok(vec![attributed_fact(
            "reference-reasoning",
            100,
            SceneFactKind::Reasoning {
                body: "Planning a playful response.".to_owned(),
            },
        )?]),
        ProofSceneCase::ReferenceThinking => Ok(vec![attributed_fact(
            "reference-thinking",
            100,
            SceneFactKind::Reasoning {
                body: "Checking `mood` for **playful** *tone* before replying.".to_owned(),
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

/// Prints the text manifest bound to one capture: the published thread
/// title plus the composer attachment count read back from the live
/// entity. Draft content has no production-visible accessor outside tests
/// (`draft()` is `cfg(test)`), so the manifest omits it rather than faking
/// a check. Pixel presence stays root's comparison, but a capture is
/// rejected when its manifest or painted-quad count is wrong before pixels
/// matter.
fn print_proof_manifest(screen: &Entity<ThreadScreen>, stem: &str, cx: &mut App) {
    let composer = screen.read(cx).composer().clone();
    let attachments = composer.read(cx).attachment_count();
    println!(
        "parity-proof manifest {stem}: title={PROOF_THREAD_TITLE:?} \
         composer_attachments={attachments}"
    );
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

/// Settles the single capture slot: records failure, marks settled, and
/// quits. Every terminal path (seed refusal, open failure, capture
/// success/failure, resize exhaustion) ends here or quits directly, so one
/// process can neither hang nor outlive its capture.
fn settle_slot(
    settled: &Rc<Cell<bool>>,
    failed_flag: &Rc<Cell<bool>>,
    failed: bool,
    cx: &mut App,
) {
    if failed {
        failed_flag.set(true);
    }
    settled.set(true);
    cx.quit();
}

/// Publishes the shell geometry bound to one capture: requested viewport
/// versus actual window bounds, the content width from actual bounds minus
/// the resolved rail, and the resulting inspector fit. Requested is never
/// labeled actual.
fn print_capture_geometry(
    slug: &str,
    requested_width: f32,
    requested_height: f32,
    actual_width: f32,
    actual_height: f32,
    scale: f32,
    content_width: f32,
) {
    let style = DesktopShellStyle::resolve(false, scale);
    let sidebar_matches = style.sidebar_width == px(DESKTOP_SIDEBAR_WIDTH_PX);
    let titlebar_matches = style.titlebar_height == px(DESKTOP_TITLEBAR_HEIGHT_PX);
    let state = if thread_inspector_visible(content_width) {
        "shown"
    } else {
        "hidden"
    };
    println!(
        "parity-proof geometry {slug}: requested={requested_width}x{requested_height} \
         actual={actual_width}x{actual_height} scale={scale} \
         sidebar={} titlebar={} content={content_width} inspector={state} \
         title=\"Parity proof thread\"",
        DESKTOP_SIDEBAR_WIDTH_PX, DESKTOP_TITLEBAR_HEIGHT_PX,
    );
    debug_assert!(sidebar_matches && titlebar_matches);
}

/// Bounds tolerance for the resize settle loop, logical pixels.
const BOUNDS_SETTLE_PX: f32 = 0.5;

/// Settle polls between resize requests, in milliseconds.
const RESIZE_POLL_MILLIS: u64 = 100;

/// Warmup redraws after the size settles before the capture draw. The
/// header entrance holds opacity 0 for 150ms plus a 150ms fade, so six
/// 100ms passes (600ms) settle it; fewer captured the intentional blank.
const WARMUP_DRAW_PASSES: u32 = 6;

/// Deterministic thread title published to every proof capture.
const PROOF_THREAD_TITLE: &str = "Parity proof thread";
const RESIZE_MAX_POLLS: u32 = 50;

/// One resize-settle poll outcome.
enum ResizePoll {
    /// Platform bounds have not reached the requested size yet.
    Waiting,
    /// Capture ran; the payload says whether it failed.
    Done(bool),
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
    /// Mounts the production thread screen through the shell lane's proof
    /// factory (gate, content width, and title in one step), then seeds one
    /// case through the controller.
    ///
    /// # Errors
    ///
    /// Returns the mount refusal, or the first controller refusal while
    /// seeding, as a message bound to the case.
    pub fn mount(
        thread_id: ThreadId,
        title: String,
        content_width_px: f32,
        case: ProofSceneCase,
        cx: &mut App,
    ) -> Result<Entity<Self>, String> {
        let screen =
            ThreadScreen::mount_proof(thread_id.clone(), title, content_width_px, cx)
                .map_err(|error| format!("mount refused: {error:?}"))?;
        seed_case(&screen, case, &thread_id, cx)?;
        print_case_manifest(&screen, case, cx);
        Ok(cx.new(|_| Self { screen }))
    }

    /// Returns the mounted thread screen for live width publication.
    #[must_use]
    pub fn screen(&self) -> &Entity<ThreadScreen> {
        &self.screen
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

/// Exact invocation: `--case <slug> --viewport <narrow|wide>` and nothing
/// else. Exactly one hidden window opens per process so the OS reclaims GPU
/// and RAM between captures; root orchestrates the 7 × 2 matrix as
/// sequential processes. Unknown or missing arguments fail closed: usage on
/// stderr and no windows opened.
const PROOF_USAGE: &str = "usage: parity-proof --case <empty|thinking|working|streaming|completed|error|longform|reference-settled|reference-thinking> --viewport <narrow|wide>";

/// Parses one explicit selection; anything else is a hard error.
fn parse_selection(args: &[String]) -> Result<ProofCapture, String> {
    if args.len() != 4 || args[0] != "--case" || args[2] != "--viewport" {
        return Err(String::from("expected exactly --case <slug> --viewport <name>"));
    }
    let case = ProofSceneCase::parse(&args[1])
        .ok_or_else(|| format!("unknown case {:?}", args[1]))?;
    let (viewport_slug, width, height) = match args[3].as_str() {
        "narrow" => ("narrow", NARROW_LOGICAL_WIDTH, NARROW_LOGICAL_HEIGHT),
        "wide" => ("wide", WIDE_LOGICAL_WIDTH, WIDE_LOGICAL_HEIGHT),
        _ => return Err(format!("unknown viewport {:?}", args[3])),
    };
    Ok(ProofCapture {
        case,
        viewport_slug,
        width,
        height,
    })
}

/// Opens the one selected hidden window, drives it to the requested size,
/// draws it synchronously with no present, saves the PNG, and maps success
/// onto the exit code.
///
/// Capture lifecycle: mount + seed through the controller, open hidden and
/// unfocused, then poll — `Window::resize` (the `show: false` open stores
/// bounds without applying them, leaving CW_USEDEFAULT) until platform
/// bounds match, syncing gpui-side scale/viewport via `bounds_changed` —
/// publish the live content width, then settle the frame with bounded
/// refreshed redraws and yields: `Window::draw` (no present),
/// `Window::render_to_image` of the final refreshed frame,
/// `ArenaClearNeeded::clear` on the same context, save, quit.
/// Requires `Window::render_to_image` (root-owned `test-support`
/// enablement) and the capture lane's shipping-wgpu readback.
#[must_use]
pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let capture = match parse_selection(&args) {
        Ok(capture) => capture,
        Err(error) => {
            eprintln!("parity-proof: {error}\n{PROOF_USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let launched = Rc::new(Cell::new(false));
    let launch_flag = Rc::clone(&launched);
    let settled = Rc::new(Cell::new(false));
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

        // No internal watchdog: an in-process timer cannot interrupt a
        // blocked UI thread, so root's external 45s guard owns the timeout.
        // The resize loop below is bounded (50 × 100ms) for the settling
        // path itself.

        let stem = capture.file_stem();
        let thread_id = ThreadId::parse(format!(
            "parity-proof-{}-{}",
            capture.case.slug(),
            capture.viewport_slug
        ))
        .expect("fixture thread id is valid");
        let shell = match ParityProofShell::mount(
            thread_id,
            String::from(PROOF_THREAD_TITLE),
            capture.width - DESKTOP_SIDEBAR_WIDTH_PX,
            capture.case,
            cx,
        ) {
            Ok(shell) => shell,
            Err(error) => {
                eprintln!("parity-proof seed failed for {stem}: {error}");
                failed.set(true);
                cx.quit();
                return;
            }
        };
        let screen = shell.read(cx).screen().clone();
            let bounds = Bounds::centered(
                None,
                size(px(capture.width), px(capture.height)),
                cx,
            );
            let caption = stem.clone();
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // Shipping caption setup so no native caption consumes
                    // client height; hidden and unfocused for proof capture.
                    titlebar: Some(TitlebarOptions {
                        title: Some(caption.clone().into()),
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                move |_, _| shell,
            );
            match opened {
                Ok(handle) => {
                    launch_flag.set(true);
                    // `show: false` stores the requested bounds but never
                    // applies them (CW_USEDEFAULT remains), so drive the
                    // hidden window to the requested size explicitly and poll
                    // until the platform bounds settle. `resize` queues
                    // SetWindowPos on the foreground executor; each poll
                    // re-reads the platform bounds, syncs gpui-side scale
                    // and viewport, and only draws once the size is valid.
                    // Bounded: 50 × 100ms, far under root's external guard.
                    let any_handle = AnyWindowHandle::from(handle);
                    let settled_flag = Rc::clone(&settled);
                    let failed_flag = Rc::clone(&failed);
                    cx.spawn(async move |cx| {
                        let clock = cx.background_executor().clone();
                        let mut cx = cx;
                        let mut warmup_draws = 0u32;
                        for _ in 0..RESIZE_MAX_POLLS {
                            let outcome =
                                cx.update_window(any_handle, |_, window, cx| {
                                    window.bounds_changed(cx);
                                    let scale = window.scale_factor();
                                    let style =
                                        DesktopShellStyle::resolve(false, scale);
                                    let actual_width =
                                        window.bounds().size.width.as_f32();
                                    let actual_height =
                                        window.bounds().size.height.as_f32();
                                    if (actual_width - capture.width).abs() > BOUNDS_SETTLE_PX
                                        || (actual_height - capture.height).abs()
                                            > BOUNDS_SETTLE_PX
                                    {
                                        window.resize(size(
                                            px(capture.width),
                                            px(capture.height),
                                        ));
                                        return ResizePoll::Waiting;
                                    }
                                    let content_width = actual_width
                                        - style.sidebar_width.as_f32();
                                    screen.update(cx, |screen, screen_cx| {
                                        if screen.set_content_width(content_width) {
                                            screen_cx.notify();
                                        }
                                    });
                                    // Settle the frame: a hidden window paints
                                    // nothing on its own, and one draw can
                                    // reuse incomplete cached paint while the
                                    // async asset/text pipeline lands (seen as
                                    // missing title/composer glyphs with
                                    // shapes intact). Every pass refreshes and
                                    // redraws with yields between; the capture
                                    // reads the final refreshed frame
                                    // directly — no second draw without a
                                    // refresh in between.
                                    window.refresh();
                                    let arena = window.draw(cx);
                                    arena.clear(cx);
                                    warmup_draws += 1;
                                    if warmup_draws < WARMUP_DRAW_PASSES {
                                        return ResizePoll::Waiting;
                                    }
                                    print_capture_geometry(
                                        &stem,
                                        capture.width,
                                        capture.height,
                                        actual_width,
                                        actual_height,
                                        scale,
                                        content_width,
                                    );
                                    print_proof_manifest(&screen, &stem, cx);
                                    // The shipping wgpu readback of the final
                                    // refreshed frame above, then save.
                                    let quads = window.painted_quads().len();
                                    let capture_result = window.render_to_image();
                                    println!(
                                        "parity-proof paint {stem}: quads={quads}"
                                    );
                                    let expected_width =
                                        (capture.width * scale).round() as u32;
                                    let expected_height =
                                        (capture.height * scale).round() as u32;
                                    let mut failed = quads == 0;
                                    if failed {
                                        eprintln!(
                                            "parity-proof paint failed for {stem}: \
                                             no quads painted"
                                        );
                                    }
                                    match capture_result {
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
                        ResizePoll::Done(failed)
                    });
                    match outcome {
                    Ok(ResizePoll::Done(failed)) => {
                        cx.update(|cx| settle_slot(&settled_flag, &failed_flag, failed, cx));
                        return;
                    }
                    Ok(ResizePoll::Waiting) => {
                        clock
                            .timer(Duration::from_millis(RESIZE_POLL_MILLIS))
                            .await;
                    }
                    Err(error) => {
                        eprintln!("parity-proof update failed for {caption}: {error:?}");
                        cx.update(|cx| {
                            failed_flag.set(true);
                            settled_flag.set(true);
                            cx.quit();
                        });
                        return;
                    }
                }
            }
            eprintln!("parity-proof resize never settled for {caption}");
            cx.update(|cx| {
                failed_flag.set(true);
                settled_flag.set(true);
                cx.quit();
            });
        })
        .detach();
                }
                Err(error) => {
                    eprintln!("parity-proof could not open its window: {error:?}");
                    failed.set(true);
                    cx.quit();
                }
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
    use super::{
        NARROW_LOGICAL_WIDTH, ProofSceneCase, case_facts, case_snapshot, parse_selection,
        reference_run_id,
    };
    use crate::conversation_delivery_machine::ConversationDeliveryEvent;
    use crate::conversation_scene::{
        ConversationScene, TurnBlock, TurnNarration, WorkGroupBlock, WorkGroupLabel,
    };
    use crate::conversation_state_machine::{
        ConversationStateController, ConversationStateEvent, SceneFactCommand, SceneFactKind,
    };
    use artisan_domain::{ThreadId, UnixMillis};

    fn thread() -> ThreadId {
        ThreadId::parse("parity-proof-test").expect("fixture thread id is valid")
    }

    fn now() -> UnixMillis {
        UnixMillis::from_millis(1_700_000_000_000)
    }

    /// Drives one case through the real aggregate exactly as the runner's
    /// `seed_case` does, minus the GPUI surface: snapshot then facts through
    /// the delivery-owned path, then the projected scene.
    fn project(case: ProofSceneCase) -> ConversationScene {
        let thread = thread();
        let mut controller = ConversationStateController::new(thread.clone());
        let snapshot = case_snapshot(case, &thread, now())
            .expect("snapshot builds")
            .expect("case carries a snapshot");
        controller
            .dispatch(ConversationStateEvent::Delivery(
                ConversationDeliveryEvent::SnapshotReceived(snapshot),
            ))
            .expect("snapshot accepted");
        for fact in case_facts(case).expect("facts build") {
            controller
                .dispatch(ConversationStateEvent::Fact(SceneFactCommand::Register(fact)))
                .expect("fact accepted");
        }
        controller.scene().expect("scene projects")
    }

    /// Returns the run-attributed session group of a single-turn scene.
    fn session_group(scene: &ConversationScene) -> &WorkGroupBlock {
        let turn = scene.turn_scenes().first().expect("one turn");
        turn.blocks()
            .iter()
            .find_map(|block| match block {
                TurnBlock::WorkGroup(group) if group.session_run.is_some() => Some(group),
                _ => None,
            })
            .expect("one run-attributed session group")
    }

    /// Returns the turn status narration of a single-turn scene.
    fn status_narration(scene: &ConversationScene) -> TurnNarration {
        let turn = scene.turn_scenes().first().expect("one turn");
        turn.blocks()
            .iter()
            .find_map(|block| match block {
                TurnBlock::TurnStatus(status) => Some(status.narration),
                _ => None,
            })
            .expect("one status row")
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
    fn selection_parses_the_exact_cli_pair() {
        let selection = parse_selection(&[
            String::from("--case"),
            String::from("thinking"),
            String::from("--viewport"),
            String::from("narrow"),
        ])
        .expect("exact pair parses");
        assert_eq!(selection.case, ProofSceneCase::Thinking);
        assert_eq!(selection.viewport_slug, "narrow");
        assert_eq!(selection.width, NARROW_LOGICAL_WIDTH);
    }

    #[test]
    fn selection_fails_closed_on_anything_else() {
        let bad = [
            vec![],
            vec![String::from("--case")],
            vec![
                String::from("--case"),
                String::from("thinking"),
                String::from("--viewport"),
                String::from("narrow"),
                String::from("extra"),
            ],
            vec![
                String::from("--case"),
                String::from("nope"),
                String::from("--viewport"),
                String::from("narrow"),
            ],
            vec![
                String::from("--case"),
                String::from("thinking"),
                String::from("--viewport"),
                String::from("huge"),
            ],
            vec![
                String::from("--viewport"),
                String::from("narrow"),
                String::from("--case"),
                String::from("thinking"),
            ],
        ];
        for args in bad {
            assert!(
                parse_selection(&args).is_err(),
                "must fail closed, got {args:?}"
            );
        }
    }

    #[test]
    fn every_slug_round_trips_through_parse() {
        for case in ProofSceneCase::all() {
            assert_eq!(ProofSceneCase::parse(case.slug()), Some(case));
        }
        assert_eq!(ProofSceneCase::parse("bogus"), None);
    }

    #[test]
    fn reference_settled_projects_session_with_thought_for_six_seconds() {
        let scene = project(ProofSceneCase::ReferenceSettled);
        let group = session_group(&scene);
        assert!(
            matches!(&group.session_run, Some(run) if *run == reference_run_id()),
            "session group carries the attributed run"
        );
        assert!(group.session.is_some(), "session carries its anchor");
        assert!(
            matches!(
                group.label,
                Some(WorkGroupLabel::ThoughtFor { millis: 6_000 })
            ),
            "settled span is six seconds"
        );
        assert!(
            group.reasoning_summary.is_none(),
            "settled rows carry no live summary line"
        );
        assert_eq!(
            status_narration(&scene),
            TurnNarration::ThoughtFor { millis: 6_000 }
        );
        let reply = scene
            .turn_scenes()
            .first()
            .expect("one turn")
            .blocks()
            .iter()
            .find_map(|block| match block {
                TurnBlock::AssistantMessage(message) => Some(message.body.clone()),
                _ => None,
            });
        assert_eq!(reply.as_deref(), Some("Whoopty! \u{1F604} Whats up?"));
    }

    #[test]
    fn reference_thinking_projects_live_markdown_summary() {
        let scene = project(ProofSceneCase::ReferenceThinking);
        let group = session_group(&scene);
        assert!(
            matches!(&group.session_run, Some(run) if *run == reference_run_id()),
            "session group carries the attributed run"
        );
        assert_eq!(
            group.reasoning_summary.as_deref(),
            Some("Checking `mood` for **playful** *tone* before replying.")
        );
        assert_eq!(status_narration(&scene), TurnNarration::Thinking);
    }

    #[test]
    fn reference_cases_share_one_attributed_run() {
        for case in [
            ProofSceneCase::ReferenceSettled,
            ProofSceneCase::ReferenceThinking,
        ] {
            assert!(
                case_snapshot(case, &thread(), now())
                    .expect("snapshot builds")
                    .is_some()
            );
            assert!(
                !case_facts(case).expect("facts build").is_empty(),
                "case {} must attribute its session facts",
                case.slug()
            );
        }
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
