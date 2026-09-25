#![expect(
    clippy::float_cmp,
    reason = "test assertions compare the exact pixel arithmetic the UI performs; an epsilon would weaken the regression coverage"
)]
use super::NATIVE_STATUS_SELECTOR;
use super::{
    NATIVE_MESSAGE_RETRY_LABEL, NATIVE_MESSAGE_RETRY_SELECTOR, NATIVE_RAIL_ADD_PROJECT_LABEL,
    NativeApplication, NativeMessageFailure, NativeMessageFlight, NativeProjectIntakeOperation,
    NativeProjectIntakeStage, NativeTestCommandSink, NativeTransportCommand, NativeTransportEvent,
    NativeViewState, PendingFailedRecovery, PickerRoute, ServiceFailure, ServiceStopStatus,
    TITLEBAR_HEADER_SELECTOR, TITLEBAR_PROJECT_FOLDER_SELECTOR, TITLEBAR_REPOSITORY_LABEL_SELECTOR,
    TITLEBAR_REPOSITORY_MARK_SELECTOR, TITLEBAR_ROUTE_TITLE_SELECTOR,
    TITLEBAR_THREAD_SEPARATOR_SELECTOR, ThreadSwitchFlight, ThreadSwitchPhase, TitlebarRepository,
    WINDOW_TITLE, create_message_request_id, intake_command, message_status_detail, picker_route,
    project_options_from_listing, ready_membership_is_valid,
};
use crate::composer::{ComposerState, DraftDisposition};
use crate::desktop_shell::{
    DESKTOP_COMPOSER_SELECTOR, DESKTOP_HOME_SELECTOR, DESKTOP_OFFLINE_SELECTOR,
    DESKTOP_SIDEBAR_SELECTOR, DESKTOP_TITLEBAR_CONTENT_INSET_PX, DESKTOP_TITLEBAR_SELECTOR,
};
use crate::native_command_menu::{
    COMMAND_MENU_DROPDOWN_SELECTOR, COMMAND_MENU_INPUT_SELECTOR, COMMAND_MENU_LIST_SELECTOR,
};
use crate::native_composer_controls::NativeComposerControlsEvent;
use crate::native_profile_usage::{
    NativeUsageAuthentication, NativeUsageCadence, NativeUsageEntry, NativeUsageQuotaSurface,
    NativeUsageReport, NativeUsageWindow,
};
use crate::native_route::{NativeRoute, SettingsRoute};
use crate::repository_mark::RepositoryHost;
use crate::{
    conversation_delivery_machine::ConversationDeliveryEffect,
    conversation_host::{ConversationHost, ConversationHostEffect},
    conversation_scene::{SceneId, TurnBlock, TurnNarration, TurnScene},
    conversation_state_machine::ConversationStateEffect,
    conversation_surface::{CONVERSATION_SURFACE_MAX_SCROLL_TARGETS, ConversationSurfaceTarget},
    conversation_view_machine::{CompletionRejection, ViewportEffect, ViewportGeneration},
    project_picker::{PickerRow, ProjectOption, ProjectPickerAction},
};
use artisan_domain::{
    AssistantBody, AssistantMessageItem, AssistantMessagePhase, ConversationCursor,
    ConversationItem, ConversationLifecycle, ConversationPatch, ConversationSnapshot,
    ConversationSubscriptionStart, ConversationTurn, DisplayName, IncrementalText, ItemId,
    ItemOrdinal, MessageBody, ObservationId, PatchBatch, PatchId, PatchSequence, ProjectId,
    ProjectListing, ProjectSummary, ReceiptDisposition, RequestId, Revision, RootPath, RunId,
    ThreadId, ThreadListing, ThreadSummary, ThreadTitle, TurnId, TurnOrdinal, UnixMillis,
    UserMessageItem,
};
use artisan_protocol::{
    ConversationSubscriptionStarted, ConversationSubscriptionStopped, QueueMessageReceipt,
    RepositoryBranchState, RepositoryHost as ProtocolRepositoryHost,
    RepositoryRemote as ProtocolRepositoryRemote, RepositorySnapshot as ProtocolRepositorySnapshot,
};
use artisan_ui::button::{
    Button, ButtonContent, ButtonSize, ButtonStyle, ButtonVariant, FocusVisibility,
};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{Context, Focusable as _, SharedString, TestAppContext};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use super::{
    CommandSendError, RefreshSwapTarget, SURFACE_HEIGHT, SURFACE_WIDTH, ServiceFailureCategory,
    ServiceFailureStage, UNNAMED_THREAD_TITLE, bind_native_actions, capitalize_label,
    command_failure, profile_usage_now_ms, titlebar_context_tone,
};

// Most application tests exercise configured hosts; explicitly supply their
// discovered fixture rather than relying on production's offline state.
fn test_application(
    window: &mut gpui::Window,
    cx: &mut Context<NativeApplication>,
) -> NativeApplication {
    let mut application = NativeApplication::new(None, window, cx);
    let catalog = crate::native_model_catalog::NativeModelCatalog::from_manifest_json(
        include_str!("../../../../tests/fixtures/model_catalog.json"),
    )
    .unwrap();
    application.host_model_catalog = Some(catalog.clone());
    application.model_selector.update(cx, |selector, cx| {
        selector.set_snapshot(catalog, cx);
        selector.set_policy(None, cx);
    });
    application
}

// Sends to a CLI-probed engine wait for a fresh authenticated usage read.
// Send-flow tests that are not about that gate start with one already
// settled, as a signed-in desktop would after its first usage refresh.
fn signed_in_test_application(
    window: &mut gpui::Window,
    cx: &mut Context<NativeApplication>,
) -> NativeApplication {
    let mut application = test_application(window, cx);
    for (engine_id, display_name) in [("codex", "Codex"), ("claude", "Claude")] {
        application
            .profile_usage
            .entries
            .push(reported_usage_entry(engine_id, display_name, Vec::new()));
    }
    application
}

// Transport commands that queue a message; snapshot refreshes and usage
// reads the application issues alongside are not part of a send's identity.
fn queued_messages(
    commands: &[super::NativeTransportCommand],
) -> Vec<&super::NativeTransportCommand> {
    commands
        .iter()
        .filter(|command| matches!(command, super::NativeTransportCommand::QueueMessage(_)))
        .collect()
}

// A desktop whose CLI engines answered their usage read as signed out:
// sends are settled by admission instead of waiting on a pending read.
fn signed_out_test_application(
    window: &mut gpui::Window,
    cx: &mut Context<NativeApplication>,
) -> NativeApplication {
    let mut application = test_application(window, cx);
    for (engine_id, display_name) in [("codex", "Codex"), ("claude", "Claude")] {
        application.profile_usage.entries.push(reported_usage_entry_with_auth(
            engine_id,
            display_name,
            NativeUsageAuthentication::Unauthenticated,
            Vec::new(),
        ));
    }
    application
}

fn project(id: &str, name: &str) -> ProjectSummary {
    ProjectSummary {
        project_id: ProjectId::parse(id).expect("project"),
        display_name: DisplayName::parse(name).expect("display name"),
        root_path: RootPath::parse(format!("/{id}")).expect("root"),
        attached_at: UnixMillis::EPOCH,
    }
}

fn thread(id: &str, project_id: &str, title: &str) -> ThreadSummary {
    ThreadSummary {
        has_started_response: true,
        has_active_work: false,
        last_message_at: None,
        thread_id: ThreadId::parse(id).expect("thread"),
        project_id: ProjectId::parse(project_id).expect("project"),
        title: ThreadTitle::parse(title).expect("title"),
        created_at: UnixMillis::EPOCH,
        updated_at: UnixMillis::EPOCH,
    }
}

fn request(id: &str) -> RequestId {
    RequestId::parse(id).expect("request")
}

fn snapshot_for(thread_id: &ThreadId, cursor: u64) -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_id.clone(),
        ConversationCursor::new(cursor),
        Vec::new(),
        Vec::new(),
        UnixMillis::EPOCH,
    )
    .expect("snapshot")
}

fn fresh_start_event(thread_id: &ThreadId, request_id: &str, cursor: u64) -> NativeTransportEvent {
    NativeTransportEvent::ConversationSubscriptionStarted {
        thread_id: thread_id.clone(),
        request_id: request(request_id),
        started: ConversationSubscriptionStarted::Fresh(ConversationSubscriptionStart::new(
            snapshot_for(thread_id, cursor),
        )),
    }
}

fn command_sink(
    outcomes: impl IntoIterator<Item = Result<(), super::CommandSendError>>,
) -> (
    NativeTestCommandSink,
    Rc<RefCell<Vec<super::NativeTransportCommand>>>,
) {
    let commands = Rc::new(RefCell::new(Vec::new()));
    let sink = NativeTestCommandSink {
        commands: commands.clone(),
        outcomes: Rc::new(RefCell::new(outcomes.into_iter().collect::<VecDeque<_>>())),
    };
    (sink, commands)
}

fn reported_usage_window(
    id: &str,
    cadence: NativeUsageCadence,
    label: Option<&str>,
    percent_used: f64,
) -> NativeUsageWindow {
    NativeUsageWindow {
        id: id.to_owned(),
        cadence,
        label: label.map(str::to_owned),
        percent_used,
        resets_at: None,
        window_minutes: None,
    }
}

fn reported_usage_entry(
    engine_id: &str,
    display_name: &str,
    windows: Vec<NativeUsageWindow>,
) -> NativeUsageEntry {
    reported_usage_entry_with_auth(
        engine_id,
        display_name,
        NativeUsageAuthentication::Authenticated,
        windows,
    )
}

fn reported_usage_entry_with_auth(
    engine_id: &str,
    display_name: &str,
    authentication: NativeUsageAuthentication,
    windows: Vec<NativeUsageWindow>,
) -> NativeUsageEntry {
    NativeUsageEntry {
        engine_id: engine_id.to_owned(),
        display_name: display_name.to_owned(),
        report: Some(NativeUsageReport {
            engine_id: engine_id.to_owned(),
            display_name: display_name.to_owned(),
            authentication,
            account_email: None,
            quota_surface: NativeUsageQuotaSurface::Supported,
            windows,
            failure: None,
        }),
        failure: None,
        fetched_at_ms: Some(super::profile_usage_now_ms().saturating_sub(60_000)),
    }
}

/// Tall real-data fixture: eight authenticated providers with three
/// cadence windows each (one at exactly zero, which stays visible),
/// replacing the old pending placeholders without weakening scroll
/// assertions.
#[expect(
    clippy::cast_precision_loss,
    reason = "the synthetic fixture counter is far below 2^53, so the usize-to-f64 conversion is exact"
)]
fn install_connected_profile_usage(
    application: &mut NativeApplication,
    sink: NativeTestCommandSink,
) {
    application.test_command_sink = Some(sink);
    let engines = [
        "profile-test-alpha",
        "profile-test-beta",
        "profile-test-gamma",
        "profile-test-delta",
        "profile-test-epsilon",
        "profile-test-zeta",
        "profile-test-eta",
        "profile-test-theta",
    ];
    for (index, engine_id) in engines.iter().enumerate() {
        let base = (index * 11) as f64;
        application.profile_usage.entries.push(reported_usage_entry(
            engine_id,
            &format!("Profile Test {index}"),
            vec![
                reported_usage_window(
                    "session",
                    NativeUsageCadence::Session,
                    None,
                    if index == 0 { 0.0 } else { 10.0 + base },
                ),
                reported_usage_window(
                    "weekly-model",
                    NativeUsageCadence::Weekly,
                    Some("Model"),
                    20.0 + base,
                ),
                reported_usage_window("monthly", NativeUsageCadence::Monthly, None, 30.0 + base),
            ],
        ));
    }
}

fn message_failure() -> ServiceFailure {
    ServiceFailure {
        stage: super::ServiceFailureStage::Request,
        category: super::ServiceFailureCategory::Peer,
    }
}

fn install_ready_message_surface(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    thread_id: ThreadId,
    draft: &str,
    sink: NativeTestCommandSink,
) {
    let host = ConversationHost::mount(thread_id.clone(), ThemeMode::Dark, &mut *cx)
        .expect("message host");
    let project = application
        .selected_project
        .clone()
        .unwrap_or_else(|| ProjectId::parse("message-project").expect("project"));
    application.selected_project = Some(project.clone());
    application.route_history.navigate(NativeRoute::Thread {
        project,
        thread: thread_id.clone(),
    });
    application.selected_thread = Some(thread_id);
    application.state = NativeViewState::Ready;
    application.conversation_host = Some(host);
    application.test_command_sink = Some(sink);
    let draft = draft.to_owned();
    application.composer.update(cx, |composer, composer_cx| {
        composer.set_disabled(false, composer_cx);
        composer.set_draft(draft);
        composer_cx.notify();
    });
    application.sync_composer_availability(cx);
}

/// Admits one harness run-terminal observation carrying `summary_title`
/// through the real engine-observation handler.
fn install_summary_title(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    thread_id: &ThreadId,
    summary_title: &str,
) {
    let terminal = artisan_domain::RunTerminalObservation::new(
        artisan_domain::ObservationId::parse("obs-summary").expect("observation"),
        artisan_domain::ObservationSequence::new(0).expect("sequence"),
        artisan_domain::RunTerminalState::Completed,
        None,
        Some(summary_title.to_owned()),
    )
    .expect("terminal observation");
    application.handle_service_event(
        NativeTransportEvent::EngineObservation(artisan_protocol::ServerEvent {
            cursor: artisan_protocol::EventCursor::new(1).expect("cursor"),
            event: artisan_domain::Event::EngineObservation(
                artisan_domain::EngineObservationEvent {
                    thread_id: thread_id.clone(),
                    observation: artisan_domain::Observation::RunTerminal(terminal),
                    attribution: None,
                },
            ),
        }),
        cx,
    );
}

/// Drives the engine-settings controller to a persisted configuration
/// built from the offline `codex-sol` policy, so send-admission tests
/// exercise the configured first-send flow instead of the unconfigured
/// block. Uses only controller-local transitions; no transport.
fn install_configured_engine_settings(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
) {
    let thread_id = application
        .selected_thread
        .clone()
        .expect("selected settings thread");
    let catalog = application
        .model_selector
        .read(cx)
        .state()
        .snapshot()
        .clone();
    let mut policy = catalog
        .selection_policy_for_model("codex-sol")
        .expect("default selection policy");
    policy.profile_id = Some("default".to_owned());
    let config = crate::composer_model_config::config_for_policy(&catalog, &policy, None)
        .expect("default policy builds a run configuration");
    application.engine_settings.select_thread(Some(&thread_id));
    let generation = application
        .engine_settings
        .prepare_settings_load()
        .expect("settings load generation");
    assert!(
        application
            .engine_settings
            .mark_settings_load_admitted(&thread_id, generation)
    );
    application.engine_settings.on_settings_loaded(
        generation,
        artisan_protocol::ThreadEngineSettingsResult::Configured {
            thread_id,
            revision: artisan_domain::EngineConfigRevision::new(1).expect("first revision"),
            config: Box::new(config),
        },
    );
    assert!(application.engine_settings.authoritative_config().is_some());
}

/// Admits a fresh authenticated Codex usage reply through the real
/// request and response handlers.
///
/// Admission observes probed readiness exactly like production instead
/// of a manually seated runnable flag: the usage read is dispatched
/// through the freshness planner and the reply settles through the
/// generation/sequence pairing.
fn admit_probed_codex_usage(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
) {
    application.ensure_profile_usage(false, Some("codex"), cx);
    let generation = application.profile_usage_generation;
    let request_seq = application
        .profile_usage
        .pending_seq("codex")
        .expect("codex usage read admitted");
    application.handle_account_usage(
        "codex",
        generation,
        request_seq,
        reported_usage_entry("codex", "Codex", Vec::new()),
        cx,
    );
}

/// Returns the request identity of the currently tracked save.
fn admitted_save_request(application: &NativeApplication) -> RequestId {
    application
        .engine_settings
        .pending_save_request_id()
        .cloned()
        .expect("admitted save")
}

/// Drives the real first-send leg without transport: the real selection
/// event proactively saves the displayed Codex policy through the shared
/// direct typed save, and the real send queues immediately without
/// holding for the save acknowledgment. Readiness comes from a usage
/// reply through the real handler â€” never from a manually seated
/// runnable flag or a manually held save. Returns the retained
/// configuration so tests can acknowledge exactly it.
fn install_admitted_first_send(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    thread_id: &ThreadId,
    draft: &str,
    sink: NativeTestCommandSink,
) -> artisan_domain::EngineRunConfig {
    install_ready_message_surface(application, cx, thread_id.to_owned(), draft, sink);
    admit_probed_codex_usage(application, cx);
    let policy = application
        .model_selector
        .read(cx)
        .state()
        .snapshot()
        .selection_policy_for_model("codex-sol")
        .expect("codex policy");
    application.handle_composer_model_event(
        &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(policy),
        cx,
    );
    // The selection auto-saves through the shared direct typed-save
    // path; the sink records the exact command production would send.
    let (pending_thread, retained) = application
        .engine_settings
        .pending_save()
        .map(|(thread, config)| (thread.clone(), config.clone()))
        .expect("selection save admitted");
    assert_eq!(pending_thread, *thread_id);
    // The send is never held for the save: it queues immediately while
    // the save is still in flight (the backend accept transaction
    // snapshots durable settings or refuses typed).
    application.begin_message_submission(cx);
    assert!(application.message_flight.is_some());
    assert!(application.composer.read(cx).is_submitting());
    retained
}

fn answer_thread() -> ThreadId {
    ThreadId::parse("thread-answer").expect("answer thread")
}

fn answer_run() -> RunId {
    RunId::parse("run-answer").expect("answer run")
}

fn answer_approval() -> ObservationId {
    ObservationId::parse("approval-1").expect("answer approval")
}

fn install_answer_surface(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    sink: NativeTestCommandSink,
) {
    let thread_id = answer_thread();
    let host =
        ConversationHost::mount(thread_id.clone(), ThemeMode::Dark, &mut *cx).expect("answer host");
    application.selected_thread = Some(thread_id);
    application.conversation_host = Some(host);
    application.test_command_sink = Some(sink);
}

fn queue_approval_answer(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
) -> RequestId {
    let host = application.conversation_host.clone().expect("answer host");
    let surface = host.read(cx).surface().clone();

    surface.update(cx, |surface, surface_cx| {
        surface.set_answer_context(answer_thread(), answer_run(), surface_cx);
        assert!(surface.submit_approval_gesture(
            "approval-1",
            &answer_approval(),
            true,
            surface_cx,
        ));
        surface.pending_answer_dispatches()[0].request_id.clone()
    })
}

fn recorded_approval(commands: &[NativeTransportCommand]) -> &artisan_domain::RespondApproval {
    assert_eq!(commands.len(), 1);
    if let NativeTransportCommand::RespondApproval(answer) = &commands[0] {
        answer
    } else {
        panic!("tick must submit an approval answer")
    }
}

/// Navigates to one engine Settings page.
fn mount_settings_engine(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    engine_id: &str,
) {
    application.navigate(
        NativeRoute::Settings {
            section: SettingsRoute::Engines,
            engine: Some(engine_id.to_owned()),
        },
        cx,
    );
}

/// Builds one contiguous echo batch carrying the canonical user item for
/// a watched send, with an item id deliberately different from the
/// Forge message id: echo matches ONLY on `source_message_id`.
/// Turn and item ordinals occupy one shared namespace.
#[expect(
    clippy::too_many_arguments,
    reason = "the test helper takes each fixture field explicitly so call sites read as data rather than a positional tuple"
)]
fn echo_batch(
    thread_id: &ThreadId,
    from: u64,
    item_id: &str,
    source_message_id: Option<&str>,
    turn: &str,
    turn_ordinal: u64,
    item_ordinal: u64,
    body: &str,
) -> PatchBatch {
    let turn = ConversationTurn {
        turn_id: TurnId::parse(turn).expect("turn"),
        ordinal: TurnOrdinal::new(turn_ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        created_at: UnixMillis::EPOCH,
        updated_at: UnixMillis::from_millis(10),
    };
    let item = ConversationItem::UserMessage(UserMessageItem {
        item_id: ItemId::parse(item_id).expect("item"),
        turn_id: turn.turn_id.clone(),
        ordinal: ItemOrdinal::new(item_ordinal),
        revision: Revision::new(0),
        lifecycle: ConversationLifecycle::Pending,
        body: MessageBody::parse(body.to_owned()).expect("user body"),
        source_message_id: source_message_id
            .map(|value| artisan_domain::MessageId::parse(value).expect("source message")),
        created_at: UnixMillis::EPOCH,
        updated_at: UnixMillis::from_millis(10),
    });
    PatchBatch::new(
        thread_id.clone(),
        ConversationCursor::new(from),
        ConversationCursor::new(from + 2),
        vec![
            ConversationPatch::TurnUpsert {
                patch_id: PatchId::parse(format!("patch-turn-{from}")).expect("patch"),
                sequence: PatchSequence::new(from + 1).expect("sequence"),
                turn,
            },
            ConversationPatch::ItemUpsert {
                patch_id: PatchId::parse(format!("patch-item-{from}")).expect("patch"),
                sequence: PatchSequence::new(from + 2).expect("sequence"),
                item,
            },
        ],
    )
    .expect("echo batch")
}

fn send_receipt_for_flight(
    application: &NativeApplication,
    thread_id: &ThreadId,
    message_id: &str,
) -> QueueMessageReceipt {
    let request_id = application
        .message_flight
        .as_ref()
        .expect("send flight")
        .request_id
        .as_str()
        .to_owned();
    first_receipt(
        &request_id,
        thread_id,
        message_id,
        ReceiptDisposition::Accepted,
    )
}

/// Reads one turn scene without touching scene files.
fn staged_turn_scene(application: &NativeApplication, cx: &gpui::App, turn: &str) -> TurnScene {
    application
        .conversation_host
        .clone()
        .expect("mounted host")
        .read(cx)
        .controller_scene()
        .expect("scene builds")
        .turn_scene(&TurnId::parse(turn).expect("turn"))
        .expect("staged turn scene")
        .clone()
}

/// Painted user bodies on one turn scene, in block order.
fn staged_user_bodies(scene: &TurnScene) -> Vec<String> {
    scene
        .blocks()
        .iter()
        .filter_map(|block| match block {
            TurnBlock::UserMessage(message) => Some(message.body.clone()),
            _ => None,
        })
        .collect()
}

/// Status narration and engine label on one turn scene, if a status row
/// paints.
fn staged_status(scene: &TurnScene) -> Option<(TurnNarration, Option<String>)> {
    scene.blocks().iter().find_map(|block| match block {
        TurnBlock::TurnStatus(status) => Some((status.narration, status.engine_label.clone())),
        _ => None,
    })
}

fn admit_message_flight(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    request_id: &str,
) {
    let (body, token) = application.composer.update(cx, |composer, _| {
        composer
            .begin_payload_submission()
            .expect("message draft admits one flight")
    });
    let thread_id = application
        .selected_thread
        .clone()
        .expect("selected message thread");
    application.message_flight = Some(NativeMessageFlight {
        thread_id,
        request_id: request(request_id),
        payload: body,
        steer_target: None,
        engine_label: None,
        token,
    });
}

fn fail_active_message(application: &mut NativeApplication, cx: &mut Context<NativeApplication>) {
    let (thread_id, request_id) = {
        let flight = application.message_flight.as_ref().expect("message flight");
        (flight.thread_id.clone(), flight.request_id.clone())
    };
    application.handle_service_event(
        NativeTransportEvent::MessageFailed {
            thread_id,
            request_id,
            failure: message_failure(),
        },
        cx,
    );
}

/// Installs a single listed thread titled with the creation placeholder.
fn install_unnamed_title_task(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    draft: &str,
    sink: NativeTestCommandSink,
) -> ThreadId {
    let thread_id = ThreadId::parse("title-task").expect("thread");
    install_ready_message_surface(application, cx, thread_id.clone(), draft, sink);
    application.thread_listing = Some(
        ThreadListing::new(vec![thread(
            "title-task",
            "message-project",
            super::UNNAMED_THREAD_TITLE,
        )])
        .expect("listing"),
    );
    thread_id
}

/// Installs one selected project option so the workspace header has a
/// project display label to paint.
fn install_project_option(application: &mut NativeApplication, name: &str) {
    application.project_options.push(ProjectOption {
        id: ProjectId::parse("message-project").expect("project"),
        name: SharedString::from(name.to_owned()),
    });
}

fn protocol_repository_snapshot() -> ProtocolRepositorySnapshot {
    ProtocolRepositorySnapshot::new(
        RepositoryBranchState::attached("main").expect("branch"),
        Some("origin".to_owned()),
        vec![
            ProtocolRepositoryRemote::new(
                ProtocolRepositoryHost::GitHub,
                "origin",
                "git@github.com:artisanstreet/varde.git",
                Some("https://github.com/artisanstreet/varde".to_owned()),
            )
            .expect("remote"),
        ],
    )
    .expect("snapshot")
}

fn first_receipt(
    request_id: &str,
    thread_id: &ThreadId,
    message_id: &str,
    disposition: ReceiptDisposition,
) -> QueueMessageReceipt {
    QueueMessageReceipt {
        request_id: artisan_domain::RequestId::parse(request_id).expect("request"),
        message_id: artisan_domain::MessageId::parse(message_id).expect("message"),
        thread_id: thread_id.clone(),
        disposition,
    }
}

/// Returns only the thread-switch protocol commands, in order.
///
/// Readiness probes and settings/registry loads share the submission
/// boundary and interleave independently; the switch assertions own the
/// exact Unsubscribe/Subscribe sequence, not the total command count.
fn switch_protocol_commands(
    commands: &Rc<RefCell<Vec<NativeTransportCommand>>>,
) -> Vec<NativeTransportCommand> {
    commands
        .borrow()
        .iter()
        .filter(|command| {
            matches!(
                command,
                NativeTransportCommand::Unsubscribe { .. }
                    | NativeTransportCommand::Subscribe { .. }
            )
        })
        .cloned()
        .collect()
}

fn prepare_thread_switch_fixture(
    application: &mut NativeApplication,
    application_cx: &mut Context<NativeApplication>,
    project_id: &ProjectId,
    source: &ThreadId,
    target: &ThreadId,
    listing: &ThreadListing,
) -> Rc<RefCell<Vec<NativeTransportCommand>>> {
    let source_host =
        ConversationHost::mount(source.clone(), ThemeMode::Dark, &mut *application_cx)
            .expect("source host");
    application.project_options = vec![ProjectOption {
        id: project_id.clone(),
        name: "Switch project".into(),
    }];
    application.selected_project = Some(project_id.clone());
    application.thread_listing = Some(listing.clone());
    application.selected_thread = Some(source.clone());
    application.conversation_host = Some(source_host.clone());
    application.active_subscription_request_id = Some(request("switch-start-a-1"));
    application.state = NativeViewState::Ready;

    let (body, token) = application
        .composer
        .update(application_cx, |composer, composer_cx| {
            composer.set_disabled(false, composer_cx);
            composer.switch_thread(source.as_str(), false, composer_cx);
            composer.set_draft("retained switch draft");
            composer.begin_payload_submission()
        })
        .expect("message flight");
    application.message_flight = Some(NativeMessageFlight {
        thread_id: source.clone(),
        request_id: request("message-switch"),
        payload: body,
        steer_target: None,
        engine_label: None,
        token,
    });

    let (sink, commands) = command_sink([Ok(()), Ok(()), Ok(()), Ok(())]);
    application.test_command_sink = Some(sink);
    application.install_thread_picker(listing.clone(), Some(source.clone()), application_cx);

    application.begin_thread_switch(target.clone(), application_cx);
    assert!(matches!(
        application
            .thread_switch_flight
            .as_ref()
            .map(|flight| &flight.phase),
        Some(ThreadSwitchPhase::AwaitingUnsubscribeStop { request_id: None })
    ));
    let protocol = switch_protocol_commands(&commands);
    assert_eq!(protocol.len(), 1);
    assert!(matches!(
        &protocol[0],
        NativeTransportCommand::Unsubscribe { thread_id } if thread_id == source
    ));
    assert_eq!(application.conversation_host.as_ref(), Some(&source_host));
    assert_eq!(application.selected_thread.as_ref(), Some(source));
    assert!(
        application
            .thread_picker
            .as_ref()
            .expect("thread picker")
            .read(application_cx)
            .state()
            .is_disabled()
    );
    assert_eq!(
        application.composer.read(application_cx).draft(),
        "retained switch draft"
    );
    commands
}

fn complete_first_thread_switch(
    application: &mut NativeApplication,
    application_cx: &mut Context<NativeApplication>,
    source: &ThreadId,
    target: &ThreadId,
    commands: &Rc<RefCell<Vec<NativeTransportCommand>>>,
) -> RequestId {
    let stop_request = request("switch-stop-a-1");
    application.handle_service_event(
        NativeTransportEvent::ConversationSubscriptionStopped {
            thread_id: source.clone(),
            request_id: stop_request.clone(),
            stopped: ConversationSubscriptionStopped {
                thread_id: source.clone(),
            },
        },
        application_cx,
    );
    // The switch protocol stays exact while readiness probes and
    // settings loads interleave on the shared boundary.
    let protocol = switch_protocol_commands(commands);
    assert_eq!(protocol.len(), 2);
    assert!(matches!(
        &protocol[0],
        NativeTransportCommand::Unsubscribe { thread_id } if thread_id == source
    ));
    assert!(matches!(
        &protocol[1],
        NativeTransportCommand::Subscribe {
            thread_id,
            after: None,
        } if thread_id == target
    ));
    assert_eq!(application.selected_thread.as_ref(), Some(target));
    assert_eq!(
        application
            .conversation_host
            .as_ref()
            .expect("target host")
            .read(application_cx)
            .controller_view()
            .delivery
            .thread_id,
        *target
    );
    assert!(matches!(
        application
            .thread_switch_flight
            .as_ref()
            .map(|flight| &flight.phase),
        Some(ThreadSwitchPhase::AwaitingSubscriptionStart { request_id: None })
    ));
    assert!(application.conversation_effects.is_empty());

    application.handle_service_event(
        fresh_start_event(target, "switch-start-b-1", 1),
        application_cx,
    );
    assert!(application.thread_switch_flight.is_none());
    assert_eq!(application.selected_thread.as_ref(), Some(target));
    assert!(matches!(&application.state, NativeViewState::Ready));
    assert!(
        application
            .conversation_host
            .as_ref()
            .expect("started target host")
            .read(application_cx)
            .controller_view()
            .delivery
            .has_snapshot
    );
    assert_eq!(switch_protocol_commands(commands).len(), 2);
    // Each thread owns its draft; A's pending text must not leak into B.
    assert_eq!(application.composer.read(application_cx).draft(), "");
    stop_request
}

fn refresh_listing_and_reject_stale_snapshot(
    application: &mut NativeApplication,
    application_cx: &mut Context<NativeApplication>,
    project_id: &ProjectId,
    source: &ThreadId,
    listing: &ThreadListing,
) {
    let refreshed_listing = ThreadListing::new(vec![
        thread("switch-thread-a", "switch-project", "A refreshed"),
        thread("switch-thread-b", "switch-project", "B refreshed"),
    ])
    .expect("refreshed listing");
    application.handle_service_event(
        NativeTransportEvent::Threads {
            project_id: project_id.clone(),
            listing: refreshed_listing.clone(),
        },
        application_cx,
    );
    assert_eq!(
        application.thread_listing.as_ref(),
        Some(&refreshed_listing)
    );
    application.handle_service_event(
        NativeTransportEvent::Threads {
            project_id: project_id.clone(),
            listing: listing.clone(),
        },
        application_cx,
    );
    assert_eq!(
        application.thread_listing.as_ref(),
        Some(&refreshed_listing)
    );

    let target_host = application.conversation_host.clone().expect("target host");
    application.handle_service_event(
        NativeTransportEvent::Snapshot(snapshot_for(source, 99)),
        application_cx,
    );
    assert_eq!(application.conversation_host.as_ref(), Some(&target_host));
    assert_eq!(application.pending_snapshot, None);
    assert!(matches!(&application.state, NativeViewState::Ready));
}

fn return_to_source_and_reject_old_generation(
    application: &mut NativeApplication,
    application_cx: &mut Context<NativeApplication>,
    source: &ThreadId,
    target: &ThreadId,
    commands: &Rc<RefCell<Vec<NativeTransportCommand>>>,
    stop_request: RequestId,
) {
    application.begin_thread_switch(source.clone(), application_cx);
    assert_eq!(switch_protocol_commands(commands).len(), 3);
    application.handle_service_event(
        NativeTransportEvent::ConversationSubscriptionStopped {
            thread_id: target.clone(),
            request_id: request("switch-stop-b-1"),
            stopped: ConversationSubscriptionStopped {
                thread_id: target.clone(),
            },
        },
        application_cx,
    );
    assert_eq!(switch_protocol_commands(commands).len(), 4);
    assert!(matches!(
        &switch_protocol_commands(commands)[3],
        NativeTransportCommand::Subscribe {
            thread_id,
            after: None,
        } if thread_id == source
    ));
    application.handle_service_event(
        fresh_start_event(source, "switch-start-a-2", 2),
        application_cx,
    );
    let returned_host = application
        .conversation_host
        .clone()
        .expect("returned host");
    assert!(application.thread_switch_flight.is_none());
    assert_eq!(application.selected_thread.as_ref(), Some(source));
    assert_eq!(
        returned_host
            .read(application_cx)
            .controller_view()
            .delivery
            .cursor,
        Some(ConversationCursor::new(2))
    );

    // The first A start and stop receipts belong to the older
    // generation. Neither may displace the current A host.
    application.handle_service_event(
        fresh_start_event(source, "switch-start-a-1", 99),
        application_cx,
    );
    application.handle_service_event(
        NativeTransportEvent::ConversationSubscriptionStopped {
            thread_id: source.clone(),
            request_id: stop_request,
            stopped: ConversationSubscriptionStopped {
                thread_id: source.clone(),
            },
        },
        application_cx,
    );
    assert_eq!(application.conversation_host.as_ref(), Some(&returned_host));
    assert_eq!(application.selected_thread.as_ref(), Some(source));
    assert_eq!(switch_protocol_commands(commands).len(), 4);
    application.reply_forge_draft("retained switch draft", application_cx);
    let composer = application.composer.read(application_cx);
    assert_eq!(composer.draft(), "retained switch draft");
}

fn seed_refresh_in_flight(application: &mut NativeApplication, thread: &ThreadId) {
    application
        .composer_queue
        .state
        .set_scope(Some(thread.clone()), 5);
    application
        .composer_queue
        .state
        .begin_queue_refresh(true, true, false, false, true)
        .expect("queue refresh in flight");
}

fn failed_reason() -> String {
    "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue".to_owned()
}

fn seed_failed_entry(application: &mut NativeApplication, thread: &ThreadId, generation: u64) {
    application
        .composer_queue
        .state
        .set_scope(Some(thread.clone()), generation);
    let token = application
        .composer_queue
        .state
        .begin_failed_refresh(true, true, false, true)
        .expect("failed refresh");
    let summary = artisan_domain::FailedMessageSummary {
        message_id: artisan_domain::MessageId::parse("message-1").expect("message"),
        thread_id: thread.clone(),
        original_request_id: request("queue-1"),
        text: Some(artisan_domain::AuthoredText::parse("hello").expect("text")),
        attachments: Vec::new(),
        accepted_at: UnixMillis::from_millis(300),
        failed_at: UnixMillis::from_millis(500),
        reason: artisan_domain::DispatchError::parse(failed_reason()).expect("diagnostic"),
    };
    let listing = artisan_domain::FailedMessageListing::new(thread.clone(), 32, 1, vec![summary])
        .expect("failed listing");
    application
        .composer_queue
        .state
        .apply_failed_listing(&token, &listing)
        .expect("failed page");
}

fn failed_policy() -> artisan_catalog::NativeModelPolicy {
    artisan_catalog::NativeModelPolicy {
        catalog_revision: "rev-1".to_owned(),
        profile_id: Some("default".to_owned()),
        engine_id: "codex".to_owned(),
        model_id: "model-a".to_owned(),
        native_model_id: "model-a".to_owned(),
        native_selection: None,
        reasoning_effort: None,
        speed: None,
        context_window: None,
        permission: None,
    }
}

fn recovery_result(
    payload: Option<artisan_domain::QueueMessagePayload>,
) -> artisan_domain::RecalledMessageResult {
    artisan_domain::RecalledMessageResult::new(
        ThreadId::parse("forge-t1").expect("old thread"),
        artisan_domain::MessageId::parse("message-1").expect("message"),
        request("queue-1"),
        payload,
    )
    .expect("recovery result")
}

#[path = "tests/forge_drafts.rs"]
mod forge_drafts;
#[path = "tests/lifecycle.rs"]
mod lifecycle;
#[path = "tests/navigation.rs"]
mod navigation;
#[path = "tests/projects.rs"]
mod projects;
#[path = "tests/project_draft_transition.rs"]
mod project_draft_transition;

#[path = "tests/retry.rs"]
mod retry;

#[path = "tests/sends.rs"]
mod sends;

#[path = "tests/settings_profile.rs"]
mod settings_profile;

#[path = "tests/profile_usage_refresh.rs"]
mod profile_usage_refresh;
