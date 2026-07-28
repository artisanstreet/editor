import { Effect, Schema } from "effect";

import {
	ProjectDirectoryList,
	ProjectDirectoryListInput,
	ProjectDirectorySelectInput,
} from "./project-directory";
import { Project, ProjectCatalogSnapshot, ProjectDetachInput } from "./project";
import { RuntimeCatalog } from "./runtime-catalog";
import {
	OrchestrationGroupListQuery,
	OrchestrationGroupListSnapshot,
} from "./orchestration-groups";
import { ThreadTranscriptQuery, ThreadTranscriptSnapshot, TranscriptEntry } from "./transcript";
import { ConversationPatchBatch, ConversationQuery, ConversationSnapshot } from "./conversation";
import {
	ImageAttachmentReference,
	ImageAttachmentUpload,
	MessageImageAttachmentQuery,
	MessageImageAttachmentQueryResult,
	UserMessageInputContentPart,
	UserMessageContentPart,
} from "./attachments";
import {
	SurfaceListQuery,
	SurfaceSnapshot,
	SurfaceUsageAggregateQuery,
	SurfaceUsageAggregateSnapshot,
	SurfaceUsageDailyQuery,
	SurfaceUsageDailySnapshot,
} from "./surfaces";

import {
	WorkspaceChangeListQuery,
	WorkspaceChangeListQueryResult,
	WorkspaceChangeDiffQuery,
	WorkspaceChangeDiffQueryResult,
	WorkspaceChangeReviewRequest,
	WorkspaceChangeRollbackRequest,
	WorkspaceChangeUpdatedEvent,
	WorkspaceConflictListQuery,
	WorkspaceConflictListQueryResult,
	WorkspaceConflictUpdatedEvent,
	WorkspaceFileReadQuery,
	WorkspaceFileReadQueryResult,
	WorkspaceFileReplaceRequest,
} from "./workspace-changes";
import {
	GitDiffQuery,
	GitDiffQueryResult,
	GitIndexStageRequest,
	GitIndexUnstageRequest,
	GitMutationResolveRequest,
	GitMutationUpdatedEvent,
	GitWorkspaceQuery,
	GitWorkspaceQueryResult,
	GitWorkspaceUpdatedEvent,
} from "./git";
import {
	ArtisanApprovalListQuery,
	ArtisanApprovalListQueryResult,
	ArtisanApprovalResolveRequest,
	ArtisanApprovalUpdatedEvent,
	ArtisanAssumptionEvent,
	ArtisanNativeActionEvent,
	ArtisanToolExecutionRequest,
	ArtisanToolInvocationEvent,
	ArtisanToolInvocationListQuery,
	ArtisanToolInvocationListQueryResult,
	ArtisanToolRegistryListQuery,
	ArtisanToolRegistryListQueryResult,
	WorkspaceFileDiscoveryQuery,
	WorkspaceFileDiscoveryQueryResult,
	WorkspaceLanguageCapabilitiesQuery,
	WorkspaceLanguageCapabilitiesQueryResult,
} from "./artisan-tools";
import {
	Identifier,
	IsoDateTime,
	JournalSequence,
	NegotiatedProtocolVersion,
	PositiveInt,
	ProtocolVersion,
	RawOrigin,
	SchemaVersion,
	StreamCursor,
	StreamSequence,
} from "./common";
import {
	ThreadActivityRecordCommand,
	ThreadArchiveCommand,
	ThreadContentErasedEvent,
	ThreadCreateCommand,
	ThreadCreateInput,
	ThreadCreatedEvent,
	ThreadErasedEvent,
	ThreadListItem,
	ThreadMetadataRefineCommand,
	ThreadMetadataUpdatedEvent,
	ThreadPinCommand,
	ProjectRef,
	ThreadProjectAffinityIgnoredEvent,
	ThreadProjectAffinityUpdatedEvent,
	ThreadProjectAssignCommand,
	ThreadProjectUnlockCommand,
	ThreadRefinementIgnoredEvent,
	ThreadRenameCommand,
	ThreadRestoreCommand,
	ThreadRetentionPolicy,
	ThreadRetentionPolicyUpdatedEvent,
	ThreadRetentionUpdateCommand,
	ThreadUnpinCommand,
} from "./thread";
import {
	GlobalGuidanceCanonicalUpdatedEvent,
	GlobalGuidanceDriftResolutionRequest,
	GlobalGuidanceProviderReconciledEvent,
	GlobalGuidanceRetryRequest,
	GlobalGuidanceSelectionRequest,
	GlobalGuidanceSelectionRequiredEvent,
	GlobalGuidanceSnapshot,
	GlobalGuidanceUpdateRequest,
} from "./guidance";
import {
	ModelBehaviourDriftResolutionRequest,
	ModelBehaviourProviderReconciledEvent,
	ModelBehaviourRetryRequest,
	ModelBehaviourSettingUpdatedEvent,
	ModelBehaviourSnapshot,
	ModelBehaviourUpdateRequest,
} from "./model-behaviour";
import {
	PreviewAssetMetadataQuery,
	PreviewBrowserLaunch,
	PreviewBrowserLaunchRequest,
	PreviewInspectionRequest,
	PreviewInspectionResult,
	PreviewInspectionSession,
	PreviewInspectionSessionUpdatedEvent,
	PreviewInspectionSessionCloseRequest,
	PreviewInspectionSessionOpenRequest,
	PreviewTarget,
	PreviewTargetGetQuery,
	PreviewTargetListQuery,
	PreviewTargetRegistration,
	PreviewTargetRemoveRequest,
	PreviewTargetStateRequest,
	PreviewTargetUpdatedEvent,
	RichLinkAssetMetadata,
	RichLinkResolution,
	RichLinkResolveQuery,
} from "./preview";

import {
	CapabilityConnectPreview,
	CapabilityConnectPreviewRequest,
	CapabilityConnectRequest,
	CapabilityDetail,
	CapabilityDriftResolutionRequest,
	CapabilityDriftOverwriteDecision,
	CapabilityDriftOverwriteRequest,
	CapabilityHealthRequest,
	CapabilityOAuthCompleteRequest,
	CapabilityOAuthBeginResult,
	CapabilityOAuthRequest,
	CapabilityOAuthTokenStatus,
	CapabilityInvocationMetadata,
	CapabilityInvocationApprovalDecision,
	CapabilityInvocationApprovalRequest,
	CapabilityInvocationRequest,
	CapabilityLifecycleRequest,
	CapabilityRegistrySnapshot,
	MarketplaceApprovalDecision,
	MarketplaceBrowseQuery,
	MarketplaceLedgerEvent,
	MarketplaceScope,
	MarketplaceEnableRequest,
	MarketplaceRemoveRequest,
	MarketplaceSyncRequest,
	NpxSkillsDiscoveryRequest,
	NpxSkillsDiscoveryResult,
	NpxSkillsImportRequest,
	RoutineDetail,
	RoutineDriftResolutionRequest,
	RoutineDriftOverwriteDecision,
	RoutineDriftOverwriteRequest,
	RoutineInstallPreview,
	RoutineInstallPreviewRequest,
	RoutineInstallRequest,
	RoutineInvocationMetadata,
	RoutineInvocationRequest,
	RoutineRollbackRequest,
	RoutineRegistrySnapshot,
} from "./marketplace";

export * from "./thread";
export * from "./guidance";
export * from "./model-behaviour";
export * from "./workspace-changes";
export * from "./git";
export * from "./artisan-tools";
export * from "./marketplace";
export * from "./preview";

const FrontendTraceMetadata = {
	message_id: Identifier,
	origin: Schema.Literal("frontend"),
	schema_version: SchemaVersion,
	sent_at: IsoDateTime,
};

const BackendTraceMetadata = {
	message_id: Identifier,
	origin: Schema.Literal("backend"),
	schema_version: SchemaVersion,
	sent_at: IsoDateTime,
};

const NegotiatedFrontendTraceMetadata = {
	...FrontendTraceMetadata,
	protocol_version: NegotiatedProtocolVersion,
};

const NegotiatedBackendTraceMetadata = {
	...BackendTraceMetadata,
	protocol_version: NegotiatedProtocolVersion,
};

const EnvironmentVariableName = Schema.String.check(
	Schema.makeFilter<string>((name) =>
		name.length === 0 || name.includes("=") || name.includes(String.fromCharCode(0))
			? "Expected a non-empty environment variable name without equals or null"
			: undefined,
	),
);

/** Queues user text for a thread or steers its active capable run. */
export const ThreadSendMessageCommand = Schema.Struct({
	attachments: Schema.optional(Schema.Array(ImageAttachmentUpload).check(Schema.isMaxLength(4))),
	content: Schema.optional(Schema.Array(UserMessageInputContentPart)),
	type: Schema.Literal("thread.send_message"),
	engine_id: Identifier,
	text: Schema.NonEmptyString,
});

export type ThreadSendMessageCommand = typeof ThreadSendMessageCommand.Type;

/** Updates the thread-local default for routing follow-ups to an active run. */
export const ThreadAutoSteerUpdateCommand = Schema.Struct({
	type: Schema.Literal("thread.auto_steer.update"),
	enabled: Schema.Boolean,
});

/** The durable, provider-neutral launch policy selected for one thread. */
export const ThreadSessionPolicy = Schema.Struct({
	/** Any engine identifier decodes; the runtime catalog rejects unregistered engines. */
	engine_id: Identifier,
	model: Schema.optional(Schema.NonEmptyString),
	reasoning_effort: Schema.Literals(["low", "medium", "high", "xhigh", "max"]),
	permission_mode: Schema.Literals(["never", "on_request"]),
	sandbox_mode: Schema.Literals(["read_only", "workspace_write"]),
	service_tier: Schema.NonEmptyString.pipe(
		Schema.optional,
		Schema.withDecodingDefault(Effect.succeed("standard")),
	),
	web_search_enabled: Schema.Boolean,
	strict_clarification: Schema.Boolean,
});
export type ThreadSessionPolicy = typeof ThreadSessionPolicy.Type;

/** Replaces the complete policy atomically, so retries have one exact intent. */
export const ThreadSessionPolicyUpdateCommand = Schema.Struct({
	type: Schema.Literal("thread.session_policy.update"),
	policy: ThreadSessionPolicy,
});

/** Resolves one Artisan intake question before a run is created. */
export const IntakeRespondQuestionCommand = Schema.Struct({
	type: Schema.Literal("intake.respond_question"),
	question_id: Identifier,
	answers: Schema.Record(Identifier, Schema.NonEmptyArray(Schema.NonEmptyString)),
});

/** Opens a durable pseudoterminal owned by a thread workspace. */
export const TerminalOpenCommand = Schema.Struct({
	type: Schema.Literal("terminal.open"),
	terminal_id: Identifier,
	workspace_id: Identifier,
	working_directory: Schema.NonEmptyString,
	executable: Schema.NonEmptyString,
	args: Schema.Array(Schema.String),
	env: Schema.optional(Schema.Record(EnvironmentVariableName, Schema.String)),
	cols: PositiveInt,
	rows: PositiveInt,
});

/** Writes UTF-8 text to one active terminal. */
export const TerminalWriteCommand = Schema.Struct({
	type: Schema.Literal("terminal.write"),
	terminal_id: Identifier,
	data: Schema.String,
});

/** Changes the visible dimensions of one active terminal. */
export const TerminalResizeCommand = Schema.Struct({
	type: Schema.Literal("terminal.resize"),
	terminal_id: Identifier,
	cols: PositiveInt,
	rows: PositiveInt,
});

/** Clears the visible screen buffer of one active terminal. */
export const TerminalClearCommand = Schema.Struct({
	type: Schema.Literal("terminal.clear"),
	terminal_id: Identifier,
});

/** Sends a termination signal to one active terminal. */
export const TerminalKillCommand = Schema.Struct({
	type: Schema.Literal("terminal.kill"),
	terminal_id: Identifier,
	signal: Schema.optional(Schema.String),
});

/** Closes one terminal and releases its native PTY resource. */
export const TerminalCloseCommand = Schema.Struct({
	type: Schema.Literal("terminal.close"),
	terminal_id: Identifier,
});

/** Starts a new PTY generation from a closed or failed terminal's saved configuration. */
export const TerminalRestartCommand = Schema.Struct({
	type: Schema.Literal("terminal.restart"),
	terminal_id: Identifier,
});

/** Keeps a terminal visible across ordinary session cleanup without changing its authority. */
export const TerminalPinCommand = Schema.Struct({
	type: Schema.Literal("terminal.pin"),
	terminal_id: Identifier,
	pinned: Schema.Boolean,
});

/** Describes the bounded resource surface delegated to one assignment. */
export const AssignmentScope = Schema.Struct({
	kind: Schema.Literals([
		"repo",
		"files",
		"branch",
		"issue",
		"test",
		"terminal",
		"document",
		"custom",
	]),
	value: Schema.NonEmptyString,
	write_access: Schema.Boolean,
});

export type AssignmentScope = typeof AssignmentScope.Type;

/** Describes the workspace boundary selected for one assignment. */
export const AssignmentWorkspace = Schema.Struct({
	workspace_id: Identifier,
	working_directory: Schema.NonEmptyString,
	isolation: Schema.Literals(["shared", "isolated"]),
});

export type AssignmentWorkspace = typeof AssignmentWorkspace.Type;

/** Describes provider-neutral permissions granted to one assignment. */
export const AssignmentPermissionPolicy = Schema.Struct({
	approval: Schema.Literals(["never", "on_request", "always"]),
	network_access: Schema.Boolean,
	write_access: Schema.Boolean,
	metadata: Schema.optional(Schema.Record(Schema.String, Schema.String)),
});

export type AssignmentPermissionPolicy = typeof AssignmentPermissionPolicy.Type;

const has_graph_control_character = (value: string) =>
	[...value].some((character) => {
		const code = character.codePointAt(0)!;

		return code <= 31 || code === 127 || (code >= 128 && code <= 159);
	});

const graph_visible_name = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		const normalized = value.trim().replace(/\s+/g, " ");

		return value.length > 256
			? "Expected at most 256 input characters"
			: has_graph_control_character(value)
				? "Expected visible text without control characters"
				: normalized.length === 0 || normalized.length > 64
					? "Expected between 1 and 64 visible characters"
					: undefined;
	}),
);

const graph_visible_role = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		const normalized = value.trim().replace(/\s+/g, " ");

		return value.length > 256
			? "Expected a role with at most 256 input characters"
			: has_graph_control_character(value)
				? "Expected a role without control characters"
				: normalized.length === 0 || normalized.length > 64
					? "Expected a role between 1 and 64 visible characters"
					: undefined;
	}),
);

const graph_status_input = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		const normalized = value.trim().replace(/\s+/g, " ");

		return value.length > 8192
			? "Expected at most 8192 status input characters"
			: has_graph_control_character(value)
				? "Expected status text without control characters"
				: normalized.length === 0 || normalized.length > 4096
					? "Expected between 1 and 4096 visible status characters"
					: undefined;
	}),
);

const graph_short_status = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 && value.length <= 160 && !has_graph_control_character(value)
			? undefined
			: "Expected at most 160 visible status characters",
	),
);

const graph_action_status = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 && value.length <= 240 && !has_graph_control_character(value)
			? undefined
			: "Expected at most 240 visible status characters",
	),
);

/**
 * Caps one graph command's durable fan-out at a level the Forge dispatcher can
 * safely absorb. The default concurrency remains four; a graph may queue up to
 * one dispatcher batch of assignments without opening more app-server processes.
 */
export const OrchestrationFanoutLimits = {
	max_assignments: 16,
	max_concurrency: 4,
} as const;

const BoundedOrchestrationConcurrency = PositiveInt.check(
	Schema.isLessThanOrEqualTo(OrchestrationFanoutLimits.max_concurrency),
);

/** Defines one bounded unit of intent in a fan-out orchestration group. */
export const AssignmentSpec = Schema.Struct({
	assignment_id: Identifier,
	agent_id: Schema.optional(Identifier),
	display_name: Schema.optional(graph_visible_name),
	role: graph_visible_role,
	scope: AssignmentScope,
	engine_id: Identifier,
	profile: Schema.NonEmptyString,
	workspace: AssignmentWorkspace,
	permission_policy: AssignmentPermissionPolicy,
	summary_contract: Schema.NonEmptyString,
	parent_node_id: Identifier,
	expected_result: Schema.NonEmptyString,
	instructions: Schema.NonEmptyString,
	max_attempts: Schema.optional(PositiveInt),
});

export type AssignmentSpec = typeof AssignmentSpec.Type;

/** Defines one explicit dependency or result-flow graph edge. */
export const GraphEdgeSpec = Schema.Struct({
	edge_id: Identifier,
	from_node_id: Identifier,
	to_node_id: Identifier,
	kind: Schema.Literals(["dependency", "result"]),
});

export type GraphEdgeSpec = typeof GraphEdgeSpec.Type;

/** Defines one explicit join node and its selected upstream assignments. */
export const JoinSpec = Schema.Struct({
	join_id: Identifier,
	strategy: Schema.Literals(["require_all", "first_success", "synthesize", "review"]),
	upstream_assignment_ids: Schema.NonEmptyArray(Identifier),
	downstream_assignment_id: Schema.optional(Identifier),
});

export type JoinSpec = typeof JoinSpec.Type;

/** Starts a durable fan-out orchestration group with at least two assignments. */
export const OrchestrationGroupStartCommand = Schema.Struct({
	type: Schema.Literal("orchestration.group.start"),
	group_id: Identifier,
	assignments: Schema.Array(AssignmentSpec).check(
		Schema.isMinLength(2),
		Schema.isMaxLength(OrchestrationFanoutLimits.max_assignments),
	),
	edges: Schema.optional(Schema.Array(GraphEdgeSpec)),
	joins: Schema.optional(Schema.Array(JoinSpec)),
	name_bank: Schema.optional(Schema.NonEmptyArray(graph_visible_name)),
	max_concurrency: Schema.optional(BoundedOrchestrationConcurrency),
});

/** Renames an Artisan-owned agent identity inside one visible group. */
export const AgentInstanceRenameCommand = Schema.Struct({
	type: Schema.Literal("agent_instance.rename"),
	group_id: Identifier,
	agent_id: Identifier,
	display_name: graph_visible_name,
});

/** Records a compact, observable status heartbeat without private reasoning. */
export const AssignmentHeartbeatCommand = Schema.Struct({
	type: Schema.Literal("assignment.heartbeat"),
	group_id: Identifier,
	assignment_id: Identifier,
	short_description: graph_status_input,
	current_action: graph_status_input,
	blocked_reason: Schema.optional(graph_status_input),
	confidence: Schema.Number.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(1),
	),
	updated_at: IsoDateTime,
});

export type AssignmentHeartbeatCommand = typeof AssignmentHeartbeatCommand.Type;

/** Steers the current run attempt for an assignment when its engine permits it. */
export const AssignmentSteerCommand = Schema.Struct({
	type: Schema.Literal("assignment.steer"),
	group_id: Identifier,
	assignment_id: Identifier,
	text: Schema.NonEmptyString,
});

/** Stops the current run attempt for an assignment when its engine permits it. */
export const AssignmentStopCommand = Schema.Struct({
	type: Schema.Literal("assignment.stop"),
	group_id: Identifier,
	assignment_id: Identifier,
});

/** Requests a pause for an assignment and records an explicit unsupported outcome. */
export const AssignmentPauseCommand = Schema.Struct({
	type: Schema.Literal("assignment.pause"),
	group_id: Identifier,
	assignment_id: Identifier,
});

/** Requests a resume for an assignment and records an explicit unsupported outcome. */
export const AssignmentResumeCommand = Schema.Struct({
	type: Schema.Literal("assignment.resume"),
	group_id: Identifier,
	assignment_id: Identifier,
});

/** Queues a fresh agent_run attempt for a terminal assignment. */
export const AssignmentRetryCommand = Schema.Struct({
	type: Schema.Literal("assignment.retry"),
	group_id: Identifier,
	assignment_id: Identifier,
});

export type AssignmentRetryCommand = typeof AssignmentRetryCommand.Type;

/** Steers the currently active run using provider-neutral user text. */
export const RunSteerCommand = Schema.Struct({
	type: Schema.Literal("run.steer"),
	text: Schema.NonEmptyString,
});

/** Requests cancellation of the currently active run. */
export const RunCancelCommand = Schema.Struct({ type: Schema.Literal("run.cancel") });

/** Closes the currently active run and releases its live resources. */
export const RunCloseCommand = Schema.Struct({ type: Schema.Literal("run.close") });

/** Resolves one durable approval interaction. */
export const RunRespondApprovalCommand = Schema.Struct({
	type: Schema.Literal("run.respond_approval"),
	approval_id: Identifier,
	approved: Schema.Boolean,
});

/** Resolves one durable question interaction. */
export const RunRespondQuestionCommand = Schema.Struct({
	type: Schema.Literal("run.respond_question"),
	answers: Schema.Record(Identifier, Schema.NonEmptyArray(Schema.NonEmptyString)),
});

/** Unions every command payload accepted by the V1 control channel. */
export const CommandPayload = Schema.Union([
	ThreadCreateCommand,
	ThreadRenameCommand,
	ThreadProjectAssignCommand,
	ThreadProjectUnlockCommand,
	ThreadMetadataRefineCommand,
	ThreadActivityRecordCommand,
	ThreadPinCommand,
	ThreadUnpinCommand,
	ThreadArchiveCommand,
	ThreadRestoreCommand,
	ThreadRetentionUpdateCommand,
	ThreadSendMessageCommand,
	ThreadAutoSteerUpdateCommand,
	ThreadSessionPolicyUpdateCommand,
	IntakeRespondQuestionCommand,
	TerminalOpenCommand,
	TerminalWriteCommand,
	TerminalResizeCommand,
	TerminalClearCommand,
	TerminalKillCommand,
	TerminalCloseCommand,
	TerminalRestartCommand,
	TerminalPinCommand,
	OrchestrationGroupStartCommand,
	AgentInstanceRenameCommand,
	AssignmentHeartbeatCommand,
	AssignmentSteerCommand,
	AssignmentStopCommand,
	AssignmentPauseCommand,
	AssignmentResumeCommand,
	AssignmentRetryCommand,
	RunSteerCommand,
	RunCancelCommand,
	RunCloseCommand,
	RunRespondApprovalCommand,
	RunRespondQuestionCommand,
]);

export type CommandPayload = typeof CommandPayload.Type;

/** Describes a typed protocol error that can be safely shown or retried by a client. */
export const ProtocolErrorDetail = Schema.Struct({
	code: Identifier,
	message: Schema.NonEmptyString,
	retryable: Schema.Boolean,
});

export type ProtocolErrorDetail = typeof ProtocolErrorDetail.Type;

/** Defines a pre-negotiation client hello without a selected protocol version. */
export const HelloEnvelope = Schema.Struct({
	...FrontendTraceMetadata,
	kind: Schema.Literal("hello"),
	payload: Schema.Struct({
		event_cursors: Schema.Array(StreamCursor),
		last_journal_sequence: JournalSequence,
		resume_mode: Schema.optional(Schema.Literals(["fresh", "resume"])),
		supported_protocol_versions: Schema.NonEmptyArray(ProtocolVersion),
	}),
});

export type HelloEnvelope = typeof HelloEnvelope.Type;

/** Confirms the negotiated version and exposes the backend's current replay cursors. */
export const WelcomeEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("welcome"),
	payload: Schema.Struct({
		connection_id: Identifier,
		current_event_cursors: Schema.Array(StreamCursor),
		heartbeat_interval_ms: PositiveInt,
		heartbeat_timeout_ms: PositiveInt,
		journal_sequence: JournalSequence,
		stream_ticket: Identifier,
	}),
});

export type WelcomeEnvelope = typeof WelcomeEnvelope.Type;

/** Carries a durable frontend command into the backend command router. */
export const CommandEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	causation_id: Schema.optional(Identifier),
	kind: Schema.Literal("command"),
	payload: CommandPayload,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type CommandEnvelope = typeof CommandEnvelope.Type;

/** Captures an accepted or duplicate command receipt persisted by the journal. */
export const AcceptedCommandReceiptPayload = Schema.Struct({
	journal_sequence: JournalSequence,
	status: Schema.Literals(["accepted", "duplicate"]),
});

/** Captures a command rejection that was not durably accepted. */
export const RejectedCommandReceiptPayload = Schema.Struct({
	error: ProtocolErrorDetail,
	status: Schema.Literal("rejected"),
});

/** Unions every possible command receipt outcome. */
export const CommandReceiptPayload = Schema.Union([
	AcceptedCommandReceiptPayload,
	RejectedCommandReceiptPayload,
]);

export type CommandReceiptPayload = typeof CommandReceiptPayload.Type;

/** Returns the durable acceptance status for a correlated command. */
export const CommandReceiptEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	causation_id: Identifier,
	correlation_id: Identifier,
	kind: Schema.Literal("command.receipt"),
	payload: CommandReceiptPayload,
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type CommandReceiptEnvelope = typeof CommandReceiptEnvelope.Type;

/** Records user text that is durably queued for a future run. */
export const ThreadMessageQueuedEvent = Schema.Struct({
	attachments: Schema.optional(Schema.Array(ImageAttachmentReference)),
	content: Schema.optional(Schema.Array(UserMessageContentPart)),
	type: Schema.Literal("thread.message_queued"),
	message_id: Identifier,
	mentioned_projects: Schema.optional(Schema.Array(ProjectRef)),
	reason: Schema.Literals([
		"no_active_run",
		"steering_rejected",
		"disabled",
		"unsupported",
		"ambiguous_target",
		"delivery_failed",
		"rejected",
	]),
	text: Schema.NonEmptyString,
	working_directory: Schema.NonEmptyString,
});

/** Records user text accepted as a steering request for a live run. */
export const ThreadMessageSteeringEvent = Schema.Struct({
	attachments: Schema.optional(Schema.Array(ImageAttachmentReference)),
	content: Schema.optional(Schema.Array(UserMessageContentPart)),
	type: Schema.Literal("thread.message_steering"),
	message_id: Identifier,
	mentioned_projects: Schema.optional(Schema.Array(ProjectRef)),
	text: Schema.NonEmptyString,
	working_directory: Schema.NonEmptyString,
});

/** Records the final durable routing decision for normal-composer text. */
export const ThreadMessageRoutedEvent = Schema.Struct({
	type: Schema.Literal("thread.message_routed"),
	message_id: Identifier,
	outcome: Schema.Literals(["steered", "queued"]),
	reason: Schema.optional(
		Schema.Literals([
			"no_active_run",
			"disabled",
			"unsupported",
			"ambiguous_target",
			"delivery_failed",
			"rejected",
		]),
	),
	run_id: Schema.optional(Identifier),
});
export type ThreadMessageRoutedEvent = typeof ThreadMessageRoutedEvent.Type;

export const ThreadSessionSnapshot = Schema.Struct({
	thread_id: Identifier,
	journal_sequence: JournalSequence,
	auto_steer_enabled: Schema.Boolean,
	policy: ThreadSessionPolicy,
	latest_intake: Schema.optional(
		Schema.Struct({
			message_id: Identifier,
			risk: Schema.Literals(["low", "material", "high", "underspecified"]),
			resolution: Schema.Literals(["proceed", "question"]),
		}),
	),
	assumptions: Schema.Array(
		Schema.Struct({ message_id: Identifier, assumption: Schema.NonEmptyString }),
	),
	pending_question: Schema.optional(
		Schema.Struct({
			question_id: Identifier,
			state: Schema.Literals(["pending", "resolved"]),
			text: Schema.NonEmptyString,
		}),
	),
	last_routing: Schema.optional(ThreadMessageRoutedEvent),
});
export type ThreadSessionSnapshot = typeof ThreadSessionSnapshot.Type;

/** Records an authoritative lifecycle state for one durable run. */
export const RunLifecycleEvent = Schema.Struct({
	type: Schema.Literal("run.lifecycle"),
	working_directory: Schema.NonEmptyString,
	state: Schema.Literals([
		"queued",
		"running",
		"waiting",
		"interrupted",
		"completed",
		"cancelled",
		"failed",
		"closed",
	]),
});

/** Persists the complete assistant response while omitting transient deltas. */
export const AssistantMessageCompletedEvent = Schema.Struct({
	type: Schema.Literal("assistant.message_completed"),
	message_id: Identifier,
	text: Schema.NonEmptyString,
});

/** Persists a provider-neutral approval request or its response. */
export const ApprovalInteractionEvent = Schema.Struct({
	type: Schema.Literal("interaction.approval"),
	approval_id: Identifier,
	approved: Schema.optional(Schema.Boolean),
	description: Schema.NonEmptyString,
	state: Schema.Literals(["requested", "resolved"]),
});

/** Persists a provider-neutral question request or its response. */
export const QuestionInteractionEvent = Schema.Struct({
	type: Schema.Literal("interaction.question"),
	answers: Schema.optional(
		Schema.Record(Identifier, Schema.NonEmptyArray(Schema.NonEmptyString)),
	),
	question_id: Identifier,
	state: Schema.Literals(["requested", "resolved"]),
	text: Schema.NonEmptyString,
	source: Schema.optional(Schema.Literals(["engine", "intake"])),
});

/** Records the explicit harness classification used before a new run starts. */
export const IntakeAssessmentEvent = Schema.Struct({
	type: Schema.Literal("intake.assessed"),
	message_id: Identifier,
	risk: Schema.Literals(["low", "material", "high", "underspecified"]),
	resolution: Schema.Literals(["proceed", "question"]),
});

/** Records a low-risk assumption that allowed the harness to proceed. */
export const IntakeAssumptionEvent = Schema.Struct({
	type: Schema.Literal("intake.assumption_recorded"),
	message_id: Identifier,
	assumption: Schema.NonEmptyString,
});

/** Records a durable session preference change for normal composer follow-ups. */
export const ThreadAutoSteerUpdatedEvent = Schema.Struct({
	type: Schema.Literal("thread.auto_steer.updated"),
	enabled: Schema.Boolean,
});

/** Records the authoritative policy used by later engine launches. */
export const ThreadSessionPolicyUpdatedEvent = Schema.Struct({
	type: Schema.Literal("thread.session_policy.updated"),
	policy: ThreadSessionPolicy,
});

/** Records an attributed filesystem mutation without retaining file content. */
export const FilesystemMutationEvent = Schema.Struct({
	destination_path: Schema.optional(Schema.NonEmptyString),
	operation: Schema.Literals(["create", "write", "delete", "rename"]),
	path: Schema.NonEmptyString,
	type: Schema.Literal("filesystem.mutation"),
});

/** Records the workspace that owns an observed process without process output. */
export const ProcessOwnershipEvent = Schema.Struct({
	source: Schema.Literals(["engine", "terminal", "artisan_tool", "git"]),
	type: Schema.Literal("process.ownership"),
	working_directory: Schema.NonEmptyString,
});

/** Records content-free Git workspace state that contributes to project affinity. */
export const GitWorkspaceObservedEvent = Schema.Struct({
	branch: Schema.optional(Schema.NonEmptyString),
	changed_file_count: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	has_diff: Schema.Boolean,
	root_path: Schema.NonEmptyString,
	type: Schema.Literal("git.workspace.observed"),
	worktree_path: Schema.NonEmptyString,
});

/** Identifies the principal that launched a durable terminal session. */
export const TerminalOwnership = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("user") }),
	Schema.Struct({ kind: Schema.Literal("agent"), agent_id: Identifier, run_id: Identifier }),
]);

/** Describes a local preview registered against a terminal, not a discovered process port. */
export const TerminalPreviewTarget = Schema.Struct({
	target_id: Identifier,
	url: Schema.NonEmptyString,
	port: PositiveInt,
	state: Schema.NonEmptyString,
});

/** Describes durable metadata for a terminal session. */
export const TerminalSession = Schema.Struct({
	terminal_id: Identifier,
	thread_id: Identifier,
	workspace_id: Identifier,
	working_directory: Schema.NonEmptyString,
	executable: Schema.NonEmptyString,
	args: Schema.Array(Schema.String),
	cols: PositiveInt,
	generation: PositiveInt,
	rows: PositiveInt,
	pid: Schema.optional(PositiveInt),
	ownership: Schema.optional(TerminalOwnership),
	pinned: Schema.optional(Schema.Boolean),
	associated_previews: Schema.optional(Schema.Array(TerminalPreviewTarget)),
	state: Schema.Literals(["opening", "active", "closed", "failed"]),
	exit_code: Schema.optional(Schema.Int),
	exit_signal: Schema.optional(Schema.Int),
	exit_reason: Schema.optional(
		Schema.Literals(["closed", "exited", "killed", "output_overflow"]),
	),
	failure: Schema.optional(Schema.NonEmptyString),
	created_at: IsoDateTime,
	updated_at: IsoDateTime,
	closed_at: Schema.optional(IsoDateTime),
});

export type TerminalSession = typeof TerminalSession.Type;

/** Enumerates the durable lifecycle language shared by graph projections. */
export const OrchestrationLifecycleState = Schema.Literals([
	"queued",
	"running",
	"waiting",
	"blocked",
	"joining",
	"summarized",
	"stopped",
	"failed",
	"complete",
]);

export type OrchestrationLifecycleState = typeof OrchestrationLifecycleState.Type;

/** Describes one visible fan-out graph owned by a parent Artisan thread. */
export const OrchestrationGroup = Schema.Struct({
	group_id: Identifier,
	thread_id: Identifier,
	coordinator_agent_id: Identifier,
	state: OrchestrationLifecycleState,
	max_concurrency: PositiveInt,
	version: PositiveInt,
	created_at: IsoDateTime,
	updated_at: IsoDateTime,
});

export type OrchestrationGroup = typeof OrchestrationGroup.Type;

/** Describes one durable Artisan identity independently from provider identity. */
export const AgentInstance = Schema.Struct({
	agent_id: Identifier,
	group_id: Identifier,
	display_name: graph_visible_name,
	role: graph_visible_role,
	created_at: IsoDateTime,
	updated_at: IsoDateTime,
});

export type AgentInstance = typeof AgentInstance.Type;

/** Describes one compact assignment heartbeat safe for visible projection use. */
export const AssignmentHeartbeat = Schema.Struct({
	short_description: graph_short_status,
	current_action: graph_action_status,
	blocked_reason: Schema.optional(graph_action_status),
	confidence: Schema.Number.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(1),
	),
	updated_at: IsoDateTime,
});

export type AssignmentHeartbeat = typeof AssignmentHeartbeat.Type;

/** Describes one durable delegated assignment and its current attempt projection. */
export const Assignment = Schema.Struct({
	assignment_id: Identifier,
	group_id: Identifier,
	agent_id: Identifier,
	role: graph_visible_role,
	scope: AssignmentScope,
	engine_id: Identifier,
	profile: Schema.NonEmptyString,
	workspace: AssignmentWorkspace,
	permission_policy: AssignmentPermissionPolicy,
	summary_contract: Schema.NonEmptyString,
	parent_node_id: Identifier,
	expected_result: Schema.NonEmptyString,
	instructions: Schema.NonEmptyString,
	state: OrchestrationLifecycleState,
	current_attempt: PositiveInt,
	max_attempts: PositiveInt,
	active_run_id: Schema.optional(Identifier),
	heartbeat: Schema.optional(AssignmentHeartbeat),
	created_at: IsoDateTime,
	updated_at: IsoDateTime,
});

export type Assignment = typeof Assignment.Type;

/** Retains provider-native run identity without treating it as Artisan identity. */
export const ProviderNativeIdentity = Schema.Struct({
	thread_id: Schema.optional(Schema.NonEmptyString),
	run_id: Schema.optional(Schema.NonEmptyString),
	display_name: Schema.optional(graph_visible_name),
});

export type ProviderNativeIdentity = typeof ProviderNativeIdentity.Type;

/** Describes one monotonic execution attempt linked to an assignment. */
export const AgentRun = Schema.Struct({
	run_id: Identifier,
	group_id: Identifier,
	assignment_id: Identifier,
	agent_id: Identifier,
	attempt: PositiveInt,
	engine_id: Identifier,
	profile: Schema.NonEmptyString,
	state: OrchestrationLifecycleState,
	native_thread_id: Schema.optional(Schema.NonEmptyString),
	native_identity: Schema.optional(ProviderNativeIdentity),
	raw_origin: Schema.optional(RawOrigin),
	last_observation_sequence: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	created_at: IsoDateTime,
	updated_at: IsoDateTime,
	completed_at: Schema.optional(IsoDateTime),
});

export type AgentRun = typeof AgentRun.Type;

/** Describes one explicit durable join node in an orchestration graph. */
export const Join = Schema.Struct({
	join_id: Identifier,
	group_id: Identifier,
	strategy: Schema.Literals(["require_all", "first_success", "synthesize", "review"]),
	state: OrchestrationLifecycleState,
	upstream_assignment_ids: Schema.NonEmptyArray(Identifier),
	downstream_assignment_id: Schema.optional(Identifier),
	selected_assignment_id: Schema.optional(Identifier),
	created_at: IsoDateTime,
	updated_at: IsoDateTime,
});

export type Join = typeof Join.Type;

/** Describes one durable graph edge without inferring topology from timestamps. */
export const GraphEdge = Schema.Struct({
	edge_id: Identifier,
	group_id: Identifier,
	from_node_id: Identifier,
	to_node_id: Identifier,
	kind: Schema.Literals(["dependency", "result"]),
});

export type GraphEdge = typeof GraphEdge.Type;

/** Describes one durable result artifact produced by an agent_run. */
export const Artifact = Schema.Struct({
	artifact_id: Identifier,
	group_id: Identifier,
	assignment_id: Identifier,
	run_id: Identifier,
	kind: Schema.Literals(["summary", "finding", "log", "file", "diff", "custom"]),
	label: Schema.NonEmptyString,
	content: Schema.optional(Schema.NonEmptyString),
	uri: Schema.optional(Schema.NonEmptyString),
	raw_origin: Schema.optional(RawOrigin),
	created_at: IsoDateTime,
});

export type Artifact = typeof Artifact.Type;

/** Provides the complete provider-neutral projection for one orchestration group. */
export const OrchestrationGraph = Schema.Struct({
	group: OrchestrationGroup,
	agent_instances: Schema.Array(AgentInstance),
	assignments: Schema.Array(Assignment),
	agent_runs: Schema.Array(AgentRun),
	joins: Schema.Array(Join),
	edges: Schema.Array(GraphEdge),
	artifacts: Schema.Array(Artifact),
	journal_sequence: JournalSequence,
});

export type OrchestrationGraph = typeof OrchestrationGraph.Type;

/** Records one durable graph-node lifecycle transition. */
export const OrchestrationGraphLifecycleEvent = Schema.Struct({
	type: Schema.Literal("orchestration.graph.lifecycle"),
	group_id: Identifier,
	node_type: Schema.Literals(["orchestration_group", "assignment", "agent_run", "join"]),
	node_id: Identifier,
	state: OrchestrationLifecycleState,
	action: Schema.NonEmptyString,
	attempt: Schema.optional(PositiveInt),
});

export type OrchestrationGraphLifecycleEvent = typeof OrchestrationGraphLifecycleEvent.Type;

/** Records a visible heartbeat projected for one assignment. */
export const AssignmentHeartbeatEvent = Schema.Struct({
	type: Schema.Literal("assignment.heartbeat"),
	group_id: Identifier,
	assignment_id: Identifier,
	heartbeat: AssignmentHeartbeat,
});

export type AssignmentHeartbeatEvent = typeof AssignmentHeartbeatEvent.Type;

/** Records a durable rename of an Artisan-owned agent identity. */
export const AgentInstanceRenamedEvent = Schema.Struct({
	type: Schema.Literal("agent_instance.renamed"),
	group_id: Identifier,
	agent_id: Identifier,
	display_name: graph_visible_name,
});

export type AgentInstanceRenamedEvent = typeof AgentInstanceRenamedEvent.Type;

/** Records an explicit capability outcome for an assignment control command. */
export const AssignmentControlEvent = Schema.Struct({
	type: Schema.Literal("assignment.control"),
	group_id: Identifier,
	assignment_id: Identifier,
	action: Schema.Literals(["steer", "stop", "pause", "resume"]),
	outcome: Schema.Literals(["accepted", "unsupported", "rejected", "ambiguous"]),
	reason: Schema.optional(Schema.NonEmptyString),
});

export type AssignmentControlEvent = typeof AssignmentControlEvent.Type;

/** Records one result artifact added to the durable graph. */
export const ArtifactRecordedEvent = Schema.Struct({
	type: Schema.Literal("artifact.recorded"),
	group_id: Identifier,
	artifact: Artifact,
});

export type ArtifactRecordedEvent = typeof ArtifactRecordedEvent.Type;

/** Records a durable terminal lifecycle transition. */
export const TerminalLifecycleEvent = Schema.Struct({
	type: Schema.Literal("terminal.lifecycle"),
	action: Schema.Literals([
		"opened",
		"written",
		"resized",
		"cleared",
		"killed",
		"closed",
		"restarted",
		"exited",
		"failed",
		"recovered",
		"pinned",
		"unpinned",
	]),
	terminal: TerminalSession,
});

export type TerminalLifecycleEvent = typeof TerminalLifecycleEvent.Type;

/** Unions every durable event payload emitted by the V1 backend. */
export const EventPayload = Schema.Union([
	ThreadCreatedEvent,
	ThreadContentErasedEvent,
	ThreadErasedEvent,
	ThreadMetadataUpdatedEvent,
	ThreadRefinementIgnoredEvent,
	ThreadProjectAffinityUpdatedEvent,
	ThreadProjectAffinityIgnoredEvent,
	ThreadRetentionPolicyUpdatedEvent,
	GlobalGuidanceCanonicalUpdatedEvent,
	GlobalGuidanceSelectionRequiredEvent,
	GlobalGuidanceProviderReconciledEvent,
	ModelBehaviourSettingUpdatedEvent,
	ModelBehaviourProviderReconciledEvent,
	MarketplaceLedgerEvent,
	WorkspaceChangeUpdatedEvent,
	WorkspaceConflictUpdatedEvent,
	ThreadMessageQueuedEvent,
	ThreadMessageSteeringEvent,
	ThreadMessageRoutedEvent,
	RunLifecycleEvent,
	AssistantMessageCompletedEvent,
	ApprovalInteractionEvent,
	QuestionInteractionEvent,
	IntakeAssessmentEvent,
	IntakeAssumptionEvent,
	ThreadAutoSteerUpdatedEvent,
	ThreadSessionPolicyUpdatedEvent,
	FilesystemMutationEvent,
	ProcessOwnershipEvent,
	GitWorkspaceObservedEvent,
	GitWorkspaceUpdatedEvent,
	GitMutationUpdatedEvent,
	TerminalLifecycleEvent,
	OrchestrationGraphLifecycleEvent,
	AssignmentHeartbeatEvent,
	AgentInstanceRenamedEvent,
	AssignmentControlEvent,
	ArtifactRecordedEvent,
	ArtisanToolInvocationEvent,
	ArtisanApprovalUpdatedEvent,
	ArtisanAssumptionEvent,
	ArtisanNativeActionEvent,
	PreviewTargetUpdatedEvent,
	PreviewInspectionSessionUpdatedEvent,
]);

export type EventPayload = typeof EventPayload.Type;

/** Delivers one durable fact with global and stream-local ordering metadata. */
export const EventEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	causation_id: Identifier,
	correlation_id: Identifier,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("event"),
	payload: EventPayload,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	sequence: StreamSequence,
	stream_id: Identifier,
	thread_id: Identifier,
});

export type EventEnvelope = typeof EventEnvelope.Type;

/** Reports a malformed or otherwise unprocessable control frame. */
export const ProtocolErrorEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	causation_id: Schema.optional(Identifier),
	correlation_id: Schema.optional(Identifier),
	kind: Schema.Literal("protocol.error"),
	payload: ProtocolErrorDetail,
	thread_id: Schema.optional(Identifier),
});

export type ProtocolErrorEnvelope = typeof ProtocolErrorEnvelope.Type;

/** Reports a negotiation failure before a protocol version can be selected. */
export const PreNegotiationProtocolErrorEnvelope = Schema.Struct({
	...BackendTraceMetadata,
	causation_id: Schema.optional(Identifier),
	correlation_id: Schema.optional(Identifier),
	kind: Schema.Literal("protocol.error"),
	payload: ProtocolErrorDetail,
});

export type PreNegotiationProtocolErrorEnvelope = typeof PreNegotiationProtocolErrorEnvelope.Type;

/** Requests the current thread-list projection from the backend. */
export const ThreadListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.list.query"),
	payload: Schema.Struct({}),
});

export type ThreadListQueryEnvelope = typeof ThreadListQueryEnvelope.Type;

/** Returns the current thread-list projection for a correlated query. */
export const ThreadListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.list.query.result"),
	payload: Schema.Struct({
		journal_sequence: JournalSequence,
		threads: Schema.Array(ThreadListItem),
	}),
});

export type ThreadListQueryResultEnvelope = typeof ThreadListQueryResultEnvelope.Type;

/** Requests a new Forge-owned thread without accepting a client-selected identity. */
export const ThreadCreateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.create.request"),
	payload: ThreadCreateInput,
});

export type ThreadCreateEnvelope = typeof ThreadCreateEnvelope.Type;

/** Returns the complete authoritative projection for the newly created thread. */
export const ThreadCreateResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.create.result"),
	payload: ThreadListItem,
});

export type ThreadCreateResultEnvelope = typeof ThreadCreateResultEnvelope.Type;

/** Lists allowed server-side roots or the children of one opaque directory id. */
export const ProjectDirectoryListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.directory.list.query"),
	payload: ProjectDirectoryListInput,
});
export type ProjectDirectoryListQueryEnvelope = typeof ProjectDirectoryListQueryEnvelope.Type;

/** Returns bounded browser-safe directory metadata. */
export const ProjectDirectoryListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.directory.list.query.result"),
	payload: ProjectDirectoryList,
});
export type ProjectDirectoryListQueryResultEnvelope =
	typeof ProjectDirectoryListQueryResultEnvelope.Type;

/** Resolves an opaque directory id to a canonical project reference. */
export const ProjectDirectorySelectEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.directory.select"),
	payload: ProjectDirectorySelectInput,
});
export type ProjectDirectorySelectEnvelope = typeof ProjectDirectorySelectEnvelope.Type;

/** Returns the canonical project selected by the server-side locator. */
export const ProjectDirectorySelectResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.directory.select.result"),
	payload: Project,
});
export type ProjectDirectorySelectResultEnvelope = typeof ProjectDirectorySelectResultEnvelope.Type;

/** Requests the complete authoritative project catalog owned by Forge. */
export const ProjectListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.list.query"),
	payload: Schema.Struct({}),
});
export type ProjectListQueryEnvelope = typeof ProjectListQueryEnvelope.Type;

/** Returns the complete authoritative Forge project catalog. */
export const ProjectListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.list.query.result"),
	payload: ProjectCatalogSnapshot,
});
export type ProjectListQueryResultEnvelope = typeof ProjectListQueryResultEnvelope.Type;

/** Detaches one Forge-owned project without accepting client filesystem data. */
export const ProjectDetachEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("project.detach"),
	payload: ProjectDetachInput,
});
export type ProjectDetachEnvelope = typeof ProjectDetachEnvelope.Type;

/** Returns the authoritative catalog after a project mutation. */
export const ProjectDetachResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("project.detach.result"),
	payload: ProjectCatalogSnapshot,
});
export type ProjectDetachResultEnvelope = typeof ProjectDetachResultEnvelope.Type;

/** Requests the immutable capability catalog exposed by this Forge process. */
export const RuntimeCatalogQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("runtime.catalog.query"),
	payload: Schema.Struct({}),
});
export type RuntimeCatalogQueryEnvelope = typeof RuntimeCatalogQueryEnvelope.Type;

/** Returns only model and harness capabilities backed by registered Forge adapters. */
export const RuntimeCatalogQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("runtime.catalog.query.result"),
	payload: RuntimeCatalog,
});
export type RuntimeCatalogQueryResultEnvelope = typeof RuntimeCatalogQueryResultEnvelope.Type;

/** Requests the current global inactive-thread retention policy. */
export const ThreadRetentionQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.retention.query"),
	payload: Schema.Struct({}),
});

export type ThreadRetentionQueryEnvelope = typeof ThreadRetentionQueryEnvelope.Type;

/** Returns the current global inactive-thread retention policy. */
export const ThreadRetentionQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.retention.query.result"),
	payload: ThreadRetentionPolicy,
});

export type ThreadRetentionQueryResultEnvelope = typeof ThreadRetentionQueryResultEnvelope.Type;

/** Updates the global inactive-thread retention policy without a synthetic thread id. */
export const ThreadRetentionUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.retention.update"),
	payload: ThreadRetentionPolicy,
});

export type ThreadRetentionUpdateEnvelope = typeof ThreadRetentionUpdateEnvelope.Type;

/** Requests the canonical global guidance content and current reconciliation state. */
export const GlobalGuidanceQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.query"),
	payload: Schema.Struct({}),
});

export type GlobalGuidanceQueryEnvelope = typeof GlobalGuidanceQueryEnvelope.Type;

/** Returns canonical guidance content without ever routing it through the event ledger. */
export const GlobalGuidanceQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("guidance.query.result"),
	payload: GlobalGuidanceSnapshot,
});

export type GlobalGuidanceQueryResultEnvelope = typeof GlobalGuidanceQueryResultEnvelope.Type;

/** Replaces the canonical guidance file through the backend-owned file workflow. */
export const GlobalGuidanceUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.update"),
	payload: GlobalGuidanceUpdateRequest,
});

export type GlobalGuidanceUpdateEnvelope = typeof GlobalGuidanceUpdateEnvelope.Type;

/** Selects one freshly rediscovered first-run provider value. */
export const GlobalGuidanceSelectionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.selection"),
	payload: GlobalGuidanceSelectionRequest,
});

export type GlobalGuidanceSelectionEnvelope = typeof GlobalGuidanceSelectionEnvelope.Type;

/** Resolves one exact provider drift observation. */
export const GlobalGuidanceDriftResolutionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.drift.resolve"),
	payload: GlobalGuidanceDriftResolutionRequest,
});

export type GlobalGuidanceDriftResolutionEnvelope =
	typeof GlobalGuidanceDriftResolutionEnvelope.Type;

/** Retries one provider's opinionated sync strategy without adding a settings toggle. */
export const GlobalGuidanceRetryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("guidance.sync.retry"),
	payload: GlobalGuidanceRetryRequest,
});

export type GlobalGuidanceRetryEnvelope = typeof GlobalGuidanceRetryEnvelope.Type;

/** Requests the current text and identity for one canonical workspace file. */
export const WorkspaceFileReadQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.file.read.query"),
	payload: WorkspaceFileReadQuery,
});

export type WorkspaceFileReadQueryEnvelope = typeof WorkspaceFileReadQueryEnvelope.Type;

/** Returns the current text and identity for one correlated workspace file query. */
export const WorkspaceFileReadQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.file.read.query.result"),
	payload: WorkspaceFileReadQueryResult,
});

export type WorkspaceFileReadQueryResultEnvelope = typeof WorkspaceFileReadQueryResultEnvelope.Type;

/** Requests an attributed replacement of one existing UTF-8 regular workspace file. */
export const WorkspaceFileReplaceEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Identifier,
	kind: Schema.Literal("workspace.file.replace"),
	payload: WorkspaceFileReplaceRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Identifier,
	thread_id: Identifier,
});

export type WorkspaceFileReplaceEnvelope = typeof WorkspaceFileReplaceEnvelope.Type;

/** Requests a review transition for one workspace change attributed to a thread. */
export const WorkspaceChangeReviewEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.change.review"),
	payload: WorkspaceChangeReviewRequest,
	thread_id: Identifier,
});

export type WorkspaceChangeReviewEnvelope = typeof WorkspaceChangeReviewEnvelope.Type;

/** Requests a guarded rollback transition for one workspace change attributed to a thread. */
export const WorkspaceChangeRollbackEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.change.rollback"),
	payload: WorkspaceChangeRollbackRequest,
	thread_id: Identifier,
});

export type WorkspaceChangeRollbackEnvelope = typeof WorkspaceChangeRollbackEnvelope.Type;

/** Requests workspace changes attributed to one thread and optionally one workspace. */
export const WorkspaceChangeListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.change.list.query"),
	payload: WorkspaceChangeListQuery,
});

export type WorkspaceChangeListQueryEnvelope = typeof WorkspaceChangeListQueryEnvelope.Type;

/** Returns the durable workspace-change projection for one correlated list query. */
export const WorkspaceChangeListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.change.list.query.result"),
	payload: WorkspaceChangeListQueryResult,
});

export type WorkspaceChangeListQueryResultEnvelope =
	typeof WorkspaceChangeListQueryResultEnvelope.Type;

export const WorkspaceConflictListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.conflict.list.query"),
	payload: WorkspaceConflictListQuery,
});
export type WorkspaceConflictListQueryEnvelope = typeof WorkspaceConflictListQueryEnvelope.Type;
export const WorkspaceConflictListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.conflict.list.query.result"),
	payload: WorkspaceConflictListQueryResult,
});
export type WorkspaceConflictListQueryResultEnvelope =
	typeof WorkspaceConflictListQueryResultEnvelope.Type;

/** Requests the unified diff for one recorded workspace change. */
export const WorkspaceChangeDiffQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.change.diff.query"),
	payload: WorkspaceChangeDiffQuery,
});

export type WorkspaceChangeDiffQueryEnvelope = typeof WorkspaceChangeDiffQueryEnvelope.Type;

/** Returns one correlated unified workspace-change diff. */
export const WorkspaceChangeDiffQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.change.diff.query.result"),
	payload: WorkspaceChangeDiffQueryResult,
});

export type WorkspaceChangeDiffQueryResultEnvelope =
	typeof WorkspaceChangeDiffQueryResultEnvelope.Type;

/** Requests the durable Git projection and unresolved mutations for one workspace. */
export const GitWorkspaceQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("git.workspace.query"),
	payload: GitWorkspaceQuery,
});

export type GitWorkspaceQueryEnvelope = typeof GitWorkspaceQueryEnvelope.Type;

/** Returns one correlated durable Git workspace projection. */
export const GitWorkspaceQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("git.workspace.query.result"),
	payload: GitWorkspaceQueryResult,
});

export type GitWorkspaceQueryResultEnvelope = typeof GitWorkspaceQueryResultEnvelope.Type;

/** Requests one bounded Git diff for an exact observed workspace snapshot. */
export const GitDiffQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("git.diff.query"),
	payload: GitDiffQuery,
});

export type GitDiffQueryEnvelope = typeof GitDiffQueryEnvelope.Type;

/** Returns one correlated ephemeral Git diff. */
export const GitDiffQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("git.diff.query.result"),
	payload: GitDiffQueryResult,
});

export type GitDiffQueryResultEnvelope = typeof GitDiffQueryResultEnvelope.Type;

/** Requests approval for staging exact paths with complete trace attribution. */
export const GitIndexStageRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("git.index.stage.request"),
	payload: GitIndexStageRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type GitIndexStageRequestEnvelope = typeof GitIndexStageRequestEnvelope.Type;

/** Requests approval for unstaging exact paths with complete trace attribution. */
export const GitIndexUnstageRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("git.index.unstage.request"),
	payload: GitIndexUnstageRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type GitIndexUnstageRequestEnvelope = typeof GitIndexUnstageRequestEnvelope.Type;

/** Resolves the approval bound to one exact Git mutation. */
export const GitMutationResolveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("git.mutation.resolve"),
	payload: GitMutationResolveRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type GitMutationResolveEnvelope = typeof GitMutationResolveEnvelope.Type;

/** Requests the curated Model Behaviour registry and current reconciliation state. */
export const ModelBehaviourQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model_behaviour.query"),
	payload: Schema.Struct({}),
});

export type ModelBehaviourQueryEnvelope = typeof ModelBehaviourQueryEnvelope.Type;

/** Returns canonical controls and content-free provider reconciliation metadata. */
export const ModelBehaviourQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("model_behaviour.query.result"),
	payload: ModelBehaviourSnapshot,
});

export type ModelBehaviourQueryResultEnvelope = typeof ModelBehaviourQueryResultEnvelope.Type;

/** Replaces one canonical global model behavior and reconciles capable providers. */
export const ModelBehaviourUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model_behaviour.update"),
	payload: ModelBehaviourUpdateRequest,
});

export type ModelBehaviourUpdateEnvelope = typeof ModelBehaviourUpdateEnvelope.Type;

/** Resolves one exact provider-native drift observation. */
export const ModelBehaviourDriftResolutionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model_behaviour.drift.resolve"),
	payload: ModelBehaviourDriftResolutionRequest,
});

export type ModelBehaviourDriftResolutionEnvelope =
	typeof ModelBehaviourDriftResolutionEnvelope.Type;

/** Retries one provider mapping without changing the canonical setting. */
export const ModelBehaviourRetryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("model_behaviour.sync.retry"),
	payload: ModelBehaviourRetryRequest,
});

export type ModelBehaviourRetryEnvelope = typeof ModelBehaviourRetryEnvelope.Type;

/** Requests progressive routine discovery without disclosing routine instructions. */
export const RoutineRegistryQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.list.query"),
	payload: MarketplaceBrowseQuery,
});
export type RoutineRegistryQueryEnvelope = typeof RoutineRegistryQueryEnvelope.Type;
export const RoutineRegistryQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.routine.list.query.result"),
	payload: RoutineRegistrySnapshot,
});
export type RoutineRegistryQueryResultEnvelope = typeof RoutineRegistryQueryResultEnvelope.Type;
export const RoutineDetailQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.detail.query"),
	payload: Schema.Struct({ routine_id: Identifier, scope: MarketplaceScope }),
});
export type RoutineDetailQueryEnvelope = typeof RoutineDetailQueryEnvelope.Type;
export const RoutineDetailQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.routine.detail.query.result"),
	payload: RoutineDetail,
});
export type RoutineDetailQueryResultEnvelope = typeof RoutineDetailQueryResultEnvelope.Type;
export const RoutineInstallPreviewEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.install.preview"),
	payload: RoutineInstallPreviewRequest,
});
export type RoutineInstallPreviewEnvelope = typeof RoutineInstallPreviewEnvelope.Type;
export const RoutineInstallPreviewResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.routine.install.preview.result"),
	payload: RoutineInstallPreview,
});
export type RoutineInstallPreviewResultEnvelope = typeof RoutineInstallPreviewResultEnvelope.Type;
/** Requests only an approval-bound install; it never performs installation by decoding this frame. */
export const RoutineInstallRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.install.request"),
	payload: RoutineInstallRequest,
});
export type RoutineInstallRequestEnvelope = typeof RoutineInstallRequestEnvelope.Type;
export const RoutineApprovalDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.install.decision"),
	payload: MarketplaceApprovalDecision,
});
export type RoutineApprovalDecisionEnvelope = typeof RoutineApprovalDecisionEnvelope.Type;
export const RoutineEnableEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.enable"),
	payload: MarketplaceEnableRequest,
});
export type RoutineEnableEnvelope = typeof RoutineEnableEnvelope.Type;
export const RoutineDisableEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.disable"),
	payload: MarketplaceEnableRequest,
});
export type RoutineDisableEnvelope = typeof RoutineDisableEnvelope.Type;
export const RoutineRemoveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.remove"),
	payload: MarketplaceRemoveRequest,
});
export type RoutineRemoveEnvelope = typeof RoutineRemoveEnvelope.Type;
export const RoutineSyncEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.sync"),
	payload: MarketplaceSyncRequest,
});
export type RoutineSyncEnvelope = typeof RoutineSyncEnvelope.Type;
export const RoutineDriftResolutionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.drift.resolve"),
	payload: RoutineDriftResolutionRequest,
});
export type RoutineDriftResolutionEnvelope = typeof RoutineDriftResolutionEnvelope.Type;
export const RoutineDriftOverwriteRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.drift.overwrite.request"),
	payload: RoutineDriftOverwriteRequest,
});
export type RoutineDriftOverwriteRequestEnvelope = typeof RoutineDriftOverwriteRequestEnvelope.Type;
export const RoutineDriftOverwriteDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.drift.overwrite.decision"),
	payload: RoutineDriftOverwriteDecision,
});
export type RoutineDriftOverwriteDecisionEnvelope =
	typeof RoutineDriftOverwriteDecisionEnvelope.Type;
export const RoutineInvokeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.invoke"),
	payload: RoutineInvocationRequest,
});
export type RoutineInvokeEnvelope = typeof RoutineInvokeEnvelope.Type;
export const RoutineInvokeResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.routine.invoke.result"),
	payload: RoutineInvocationMetadata,
});
export type RoutineInvokeResultEnvelope = typeof RoutineInvokeResultEnvelope.Type;
export const RoutineRollbackEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.routine.rollback"),
	payload: RoutineRollbackRequest,
});
export type RoutineRollbackEnvelope = typeof RoutineRollbackEnvelope.Type;
export const NpxSkillsDiscoverEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.npx_skills.discover"),
	payload: NpxSkillsDiscoveryRequest,
});
export type NpxSkillsDiscoverEnvelope = typeof NpxSkillsDiscoverEnvelope.Type;
export const NpxSkillsDiscoverResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.npx_skills.discover.result"),
	payload: NpxSkillsDiscoveryResult,
});
export type NpxSkillsDiscoverResultEnvelope = typeof NpxSkillsDiscoverResultEnvelope.Type;
export const NpxSkillsImportEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.npx_skills.import.request"),
	payload: NpxSkillsImportRequest,
});
export type NpxSkillsImportEnvelope = typeof NpxSkillsImportEnvelope.Type;

export const CapabilityRegistryQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.list.query"),
	payload: MarketplaceBrowseQuery,
});
export type CapabilityRegistryQueryEnvelope = typeof CapabilityRegistryQueryEnvelope.Type;
export const CapabilityRegistryQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.list.query.result"),
	payload: CapabilityRegistrySnapshot,
});
export type CapabilityRegistryQueryResultEnvelope =
	typeof CapabilityRegistryQueryResultEnvelope.Type;
export const CapabilityDetailQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.detail.query"),
	payload: Schema.Struct({ capability_id: Identifier, scope: MarketplaceScope }),
});
export type CapabilityDetailQueryEnvelope = typeof CapabilityDetailQueryEnvelope.Type;
export const CapabilityDetailQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.detail.query.result"),
	payload: CapabilityDetail,
});
export type CapabilityDetailQueryResultEnvelope = typeof CapabilityDetailQueryResultEnvelope.Type;
export const CapabilityConnectPreviewEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.connect.preview"),
	payload: CapabilityConnectPreviewRequest,
});
export type CapabilityConnectPreviewEnvelope = typeof CapabilityConnectPreviewEnvelope.Type;
export const CapabilityConnectPreviewResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.connect.preview.result"),
	payload: CapabilityConnectPreview,
});
export type CapabilityConnectPreviewResultEnvelope =
	typeof CapabilityConnectPreviewResultEnvelope.Type;
/** Requests an approval-bound connect; start/connect happens only after a separate decision. */
export const CapabilityConnectRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.connect.request"),
	payload: CapabilityConnectRequest,
});
export type CapabilityConnectRequestEnvelope = typeof CapabilityConnectRequestEnvelope.Type;
export const CapabilityApprovalDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.connect.decision"),
	payload: MarketplaceApprovalDecision,
});
export type CapabilityApprovalDecisionEnvelope = typeof CapabilityApprovalDecisionEnvelope.Type;

const CapabilityActionEnvelope = <const Kind extends string>(kind: Kind) =>
	Schema.Struct({
		...NegotiatedFrontendTraceMetadata,
		kind: Schema.Literal(kind),
		payload: CapabilityLifecycleRequest,
	});
export const CapabilityStartEnvelope = CapabilityActionEnvelope("marketplace.capability.start");
export const CapabilityReconnectEnvelope = CapabilityActionEnvelope(
	"marketplace.capability.reconnect",
);
export const CapabilityDisconnectEnvelope = CapabilityActionEnvelope(
	"marketplace.capability.disconnect",
);
export const CapabilityRestartEnvelope = CapabilityActionEnvelope("marketplace.capability.restart");
export const CapabilityUninstallEnvelope = CapabilityActionEnvelope(
	"marketplace.capability.uninstall",
);
export type CapabilityStartEnvelope = typeof CapabilityStartEnvelope.Type;
export type CapabilityReconnectEnvelope = typeof CapabilityReconnectEnvelope.Type;
export type CapabilityDisconnectEnvelope = typeof CapabilityDisconnectEnvelope.Type;
export type CapabilityRestartEnvelope = typeof CapabilityRestartEnvelope.Type;
export type CapabilityUninstallEnvelope = typeof CapabilityUninstallEnvelope.Type;
export const CapabilityHealthEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.health"),
	payload: CapabilityHealthRequest,
});
export type CapabilityHealthEnvelope = typeof CapabilityHealthEnvelope.Type;
export const CapabilityEnableEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.enable"),
	payload: MarketplaceEnableRequest,
});
export const CapabilityDisableEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.disable"),
	payload: MarketplaceEnableRequest,
});
export const CapabilityRemoveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.remove"),
	payload: MarketplaceRemoveRequest,
});
export const CapabilitySyncEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.sync"),
	payload: MarketplaceSyncRequest,
});
export const CapabilityDriftResolutionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.drift.resolve"),
	payload: CapabilityDriftResolutionRequest,
});
export const CapabilityInvokeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.invoke"),
	payload: CapabilityInvocationRequest,
});
export type CapabilityEnableEnvelope = typeof CapabilityEnableEnvelope.Type;
export type CapabilityDisableEnvelope = typeof CapabilityDisableEnvelope.Type;
export type CapabilityRemoveEnvelope = typeof CapabilityRemoveEnvelope.Type;
export type CapabilitySyncEnvelope = typeof CapabilitySyncEnvelope.Type;
export type CapabilityDriftResolutionEnvelope = typeof CapabilityDriftResolutionEnvelope.Type;
export const CapabilityDriftOverwriteRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.drift.overwrite.request"),
	payload: CapabilityDriftOverwriteRequest,
});
export type CapabilityDriftOverwriteRequestEnvelope =
	typeof CapabilityDriftOverwriteRequestEnvelope.Type;
export const CapabilityDriftOverwriteDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.drift.overwrite.decision"),
	payload: CapabilityDriftOverwriteDecision,
});
export type CapabilityDriftOverwriteDecisionEnvelope =
	typeof CapabilityDriftOverwriteDecisionEnvelope.Type;
export const CapabilityInvocationApprovalRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.invoke.request"),
	payload: CapabilityInvocationApprovalRequest,
});
export type CapabilityInvocationApprovalRequestEnvelope =
	typeof CapabilityInvocationApprovalRequestEnvelope.Type;
export const CapabilityInvocationApprovalDecisionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.invoke.decision"),
	payload: CapabilityInvocationApprovalDecision,
});
export type CapabilityInvocationApprovalDecisionEnvelope =
	typeof CapabilityInvocationApprovalDecisionEnvelope.Type;
export type CapabilityInvokeEnvelope = typeof CapabilityInvokeEnvelope.Type;
export const CapabilityInvokeResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.invoke.result"),
	payload: CapabilityInvocationMetadata,
});
export type CapabilityInvokeResultEnvelope = typeof CapabilityInvokeResultEnvelope.Type;
export const CapabilityOAuthBeginEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.begin"),
	payload: CapabilityOAuthRequest,
});
export const CapabilityOAuthBeginResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.oauth.begin.result"),
	payload: CapabilityOAuthBeginResult,
});
export const CapabilityOAuthCompleteEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.complete"),
	payload: CapabilityOAuthCompleteRequest,
});
export const CapabilityOAuthRefreshEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.refresh"),
	payload: CapabilityOAuthRequest,
});
export const CapabilityOAuthRevokeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.revoke"),
	payload: CapabilityOAuthRequest,
});
export const CapabilityOAuthTokenStatusEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("marketplace.capability.oauth.status.query"),
	payload: CapabilityOAuthRequest,
});
export const CapabilityOAuthTokenStatusResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("marketplace.capability.oauth.status.query.result"),
	payload: CapabilityOAuthTokenStatus,
});
export type CapabilityOAuthBeginEnvelope = typeof CapabilityOAuthBeginEnvelope.Type;
export type CapabilityOAuthBeginResultEnvelope = typeof CapabilityOAuthBeginResultEnvelope.Type;
export type CapabilityOAuthCompleteEnvelope = typeof CapabilityOAuthCompleteEnvelope.Type;
export type CapabilityOAuthRefreshEnvelope = typeof CapabilityOAuthRefreshEnvelope.Type;
export type CapabilityOAuthRevokeEnvelope = typeof CapabilityOAuthRevokeEnvelope.Type;
export type CapabilityOAuthTokenStatusEnvelope = typeof CapabilityOAuthTokenStatusEnvelope.Type;
export type CapabilityOAuthTokenStatusResultEnvelope =
	typeof CapabilityOAuthTokenStatusResultEnvelope.Type;

/** Describes the durable work state coordinated for one thread. */
export const ThreadWorkItem = Schema.Struct({
	agent_id: Identifier,
	display_name: Schema.NonEmptyString,
	engine_id: Identifier,
	native_thread_id: Schema.optional(Identifier),
	role: Schema.NonEmptyString,
	run_id: Identifier,
	status: Schema.Literals([
		"queued",
		"running",
		"waiting",
		"interrupted",
		"completed",
		"cancelled",
		"failed",
		"closed",
	]),
});

export type ThreadWorkItem = typeof ThreadWorkItem.Type;

/** Requests the current durable coordinator work for one thread. */
export const ThreadWorkQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.work.query"),
	payload: Schema.Struct({ thread_id: Identifier }),
});

export type ThreadWorkQueryEnvelope = typeof ThreadWorkQueryEnvelope.Type;

/** Returns the durable coordinator work for one thread. */
export const ThreadWorkQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.work.query.result"),
	payload: Schema.Struct({ work: Schema.optional(ThreadWorkItem) }),
});

export type ThreadWorkQueryResultEnvelope = typeof ThreadWorkQueryResultEnvelope.Type;

/** Requests terminal metadata for one thread workspace. */
export const TerminalListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("terminal.list.query"),
	payload: Schema.Struct({ thread_id: Identifier, workspace_id: Identifier }),
});

export type TerminalListQueryEnvelope = typeof TerminalListQueryEnvelope.Type;

/** Returns durable terminal metadata without replaying transient PTY output. */
export const TerminalListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("terminal.list.query.result"),
	payload: Schema.Struct({ terminals: Schema.Array(TerminalSession) }),
});

export type TerminalListQueryResultEnvelope = typeof TerminalListQueryResultEnvelope.Type;

/** Requests the complete durable projection for one orchestration group. */
export const OrchestrationGraphQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("orchestration.graph.query"),
	payload: Schema.Struct({ group_id: Identifier }),
});

export type OrchestrationGraphQueryEnvelope = typeof OrchestrationGraphQueryEnvelope.Type;

/** Lists explicit local preview targets without rendering their pages in Artisan. */
export const PreviewTargetListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.list.query"),
	payload: PreviewTargetListQuery,
});
export type PreviewTargetListQueryEnvelope = typeof PreviewTargetListQueryEnvelope.Type;

export const PreviewTargetListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.target.list.query.result"),
	payload: Schema.Struct({ targets: Schema.Array(PreviewTarget) }),
});
export type PreviewTargetListQueryResultEnvelope = typeof PreviewTargetListQueryResultEnvelope.Type;

export const PreviewTargetGetQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.get.query"),
	payload: PreviewTargetGetQuery,
});
export type PreviewTargetGetQueryEnvelope = typeof PreviewTargetGetQueryEnvelope.Type;

export const PreviewTargetGetQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.target.get.query.result"),
	payload: PreviewTarget,
});
export type PreviewTargetGetQueryResultEnvelope = typeof PreviewTargetGetQueryResultEnvelope.Type;

export const PreviewTargetRegisterEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.register"),
	payload: PreviewTargetRegistration,
});
export type PreviewTargetRegisterEnvelope = typeof PreviewTargetRegisterEnvelope.Type;

export const PreviewTargetProbeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.probe"),
	payload: PreviewTargetGetQuery,
});
export type PreviewTargetProbeEnvelope = typeof PreviewTargetProbeEnvelope.Type;

export const PreviewTargetStateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.state"),
	payload: PreviewTargetStateRequest,
});
export type PreviewTargetStateEnvelope = typeof PreviewTargetStateEnvelope.Type;

export const PreviewTargetRemoveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.target.remove"),
	payload: PreviewTargetRemoveRequest,
});
export type PreviewTargetRemoveEnvelope = typeof PreviewTargetRemoveEnvelope.Type;

/** Returns a correlated target mutation result; removal returns the removed target for lifecycle attribution. */
export const PreviewTargetMutationResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.target.mutation.result"),
	payload: PreviewTarget,
});
export type PreviewTargetMutationResultEnvelope = typeof PreviewTargetMutationResultEnvelope.Type;

export const RichLinkResolveQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.rich_link.resolve.query"),
	payload: RichLinkResolveQuery,
});
export type RichLinkResolveQueryEnvelope = typeof RichLinkResolveQueryEnvelope.Type;

export const RichLinkResolveQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.rich_link.resolve.query.result"),
	payload: RichLinkResolution,
});
export type RichLinkResolveQueryResultEnvelope = typeof RichLinkResolveQueryResultEnvelope.Type;

export const PreviewAssetMetadataQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.asset.metadata.query"),
	payload: PreviewAssetMetadataQuery,
});
export type PreviewAssetMetadataQueryEnvelope = typeof PreviewAssetMetadataQueryEnvelope.Type;

export const PreviewAssetMetadataQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.asset.metadata.query.result"),
	payload: RichLinkAssetMetadata,
});
export type PreviewAssetMetadataQueryResultEnvelope =
	typeof PreviewAssetMetadataQueryResultEnvelope.Type;

export const PreviewBrowserLaunchEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.browser.launch"),
	payload: PreviewBrowserLaunchRequest,
});
export type PreviewBrowserLaunchEnvelope = typeof PreviewBrowserLaunchEnvelope.Type;

export const PreviewBrowserLaunchResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.browser.launch.result"),
	payload: PreviewBrowserLaunch,
});
export type PreviewBrowserLaunchResultEnvelope = typeof PreviewBrowserLaunchResultEnvelope.Type;

export const PreviewInspectionSessionOpenEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.inspection.open"),
	payload: PreviewInspectionSessionOpenRequest,
});
export type PreviewInspectionSessionOpenEnvelope = typeof PreviewInspectionSessionOpenEnvelope.Type;

export const PreviewInspectionSessionOpenResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.inspection.open.result"),
	payload: PreviewInspectionSession,
});
export type PreviewInspectionSessionOpenResultEnvelope =
	typeof PreviewInspectionSessionOpenResultEnvelope.Type;

export const PreviewInspectionEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.inspection.inspect"),
	payload: PreviewInspectionRequest,
});
export type PreviewInspectionEnvelope = typeof PreviewInspectionEnvelope.Type;

export const PreviewInspectionResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.inspection.inspect.result"),
	payload: PreviewInspectionResult,
});
export type PreviewInspectionResultEnvelope = typeof PreviewInspectionResultEnvelope.Type;

export const PreviewInspectionSessionCloseEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.inspection.close"),
	payload: PreviewInspectionSessionCloseRequest,
});
export type PreviewInspectionSessionCloseEnvelope =
	typeof PreviewInspectionSessionCloseEnvelope.Type;

export const PreviewInspectionSessionCloseResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.inspection.close.result"),
	payload: PreviewInspectionSession,
});
export type PreviewInspectionSessionCloseResultEnvelope =
	typeof PreviewInspectionSessionCloseResultEnvelope.Type;

/** Returns one provider-neutral orchestration graph projection. */
export const OrchestrationGraphQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("orchestration.graph.query.result"),
	payload: Schema.Struct({ graph: OrchestrationGraph }),
});

export type OrchestrationGraphQueryResultEnvelope =
	typeof OrchestrationGraphQueryResultEnvelope.Type;

/** Requests renderer-safe, bounded journal facts for one thread. */
export const ThreadTranscriptQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.transcript.query"),
	payload: ThreadTranscriptQuery,
});
export type ThreadTranscriptQueryEnvelope = typeof ThreadTranscriptQueryEnvelope.Type;

export const ThreadTranscriptQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.transcript.query.result"),
	payload: ThreadTranscriptSnapshot,
});
export type ThreadTranscriptQueryResultEnvelope = typeof ThreadTranscriptQueryResultEnvelope.Type;

/** Requests the canonical renderer-ready conversation projection for one thread. */
export const ConversationQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("conversation.query"),
	payload: ConversationQuery,
});
export type ConversationQueryEnvelope = typeof ConversationQueryEnvelope.Type;

export const ConversationQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("conversation.query.result"),
	payload: ConversationSnapshot,
});
export type ConversationQueryResultEnvelope = typeof ConversationQueryResultEnvelope.Type;

/** Reads one persisted user image without widening conversation snapshots or events. */
export const MessageImageAttachmentQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("message.image_attachment.query"),
	payload: MessageImageAttachmentQuery,
});
export type MessageImageAttachmentQueryEnvelope = typeof MessageImageAttachmentQueryEnvelope.Type;

export const MessageImageAttachmentQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("message.image_attachment.query.result"),
	payload: MessageImageAttachmentQueryResult,
});
export type MessageImageAttachmentQueryResultEnvelope =
	typeof MessageImageAttachmentQueryResultEnvelope.Type;

/** Discovers a thread's current and historic orchestration groups without a known id. */
export const OrchestrationGroupListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("orchestration.group.list.query"),
	payload: OrchestrationGroupListQuery,
});
export type OrchestrationGroupListQueryEnvelope = typeof OrchestrationGroupListQueryEnvelope.Type;

export const OrchestrationGroupListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("orchestration.group.list.query.result"),
	payload: OrchestrationGroupListSnapshot,
});
export type OrchestrationGroupListQueryResultEnvelope =
	typeof OrchestrationGroupListQueryResultEnvelope.Type;

export const ThreadSessionQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.session.query"),
	payload: Schema.Struct({ thread_id: Identifier }),
});
export type ThreadSessionQueryEnvelope = typeof ThreadSessionQueryEnvelope.Type;
export const ThreadSessionQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.session.query.result"),
	payload: ThreadSessionSnapshot,
});
export type ThreadSessionQueryResultEnvelope = typeof ThreadSessionQueryResultEnvelope.Type;

export const SurfaceListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("surface.list.query"),
	payload: SurfaceListQuery,
});
export type SurfaceListQueryEnvelope = typeof SurfaceListQueryEnvelope.Type;
export const SurfaceListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("surface.list.query.result"),
	payload: SurfaceSnapshot,
});
export type SurfaceListQueryResultEnvelope = typeof SurfaceListQueryResultEnvelope.Type;

export const SurfaceUsageAggregateQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("surface.usage.aggregate.query"),
	payload: SurfaceUsageAggregateQuery,
});
export type SurfaceUsageAggregateQueryEnvelope = typeof SurfaceUsageAggregateQueryEnvelope.Type;
export const SurfaceUsageAggregateQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("surface.usage.aggregate.query.result"),
	payload: SurfaceUsageAggregateSnapshot,
});
export type SurfaceUsageAggregateQueryResultEnvelope =
	typeof SurfaceUsageAggregateQueryResultEnvelope.Type;

export const SurfaceUsageDailyQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("surface.usage.daily.query"),
	payload: SurfaceUsageDailyQuery,
});
export type SurfaceUsageDailyQueryEnvelope = typeof SurfaceUsageDailyQueryEnvelope.Type;
export const SurfaceUsageDailyQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("surface.usage.daily.query.result"),
	payload: SurfaceUsageDailySnapshot,
});
export type SurfaceUsageDailyQueryResultEnvelope = typeof SurfaceUsageDailyQueryResultEnvelope.Type;

/** Requests ordered updates for the thread-list projection. */
export const SubscribeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("subscribe"),
	payload: Schema.Union([
		Schema.Struct({ type: Schema.Literal("project.list") }),
		Schema.Struct({ type: Schema.Literal("thread.list") }),
		Schema.Struct({ type: Schema.Literal("orchestration.graph"), group_id: Identifier }),
		Schema.Struct({ type: Schema.Literal("thread.transcript"), thread_id: Identifier }),
		Schema.Struct({ type: Schema.Literal("conversation"), thread_id: Identifier }),
		Schema.Struct({
			type: Schema.Literal("orchestration.group.list"),
			thread_id: Identifier,
			include_terminal: Schema.Boolean,
		}),
		Schema.Struct({ type: Schema.Literal("thread.session"), thread_id: Identifier }),
		Schema.Struct({ type: Schema.Literal("surface.list"), query: SurfaceListQuery }),
		Schema.Struct({ type: Schema.Literal("workspace.conflict.list"), thread_id: Identifier }),
		Schema.Struct({
			type: Schema.Literal("surface.usage.aggregate"),
			query: SurfaceUsageAggregateQuery,
		}),
	]),
	subscription_id: Identifier,
});

export type SubscribeEnvelope = typeof SubscribeEnvelope.Type;

/** Stops delivery for a previously established projection subscription. */
export const UnsubscribeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("unsubscribe"),
	payload: Schema.Struct({}),
	subscription_id: Identifier,
});

export type UnsubscribeEnvelope = typeof UnsubscribeEnvelope.Type;

/** Confirms that the backend has established an ordered projection subscription. */
export const SubscriptionStartedEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("subscription.started"),
	payload: Schema.Struct({
		stream_id: Identifier,
	}),
	subscription_id: Identifier,
});

export type SubscriptionStartedEnvelope = typeof SubscriptionStartedEnvelope.Type;

/** Confirms that the backend has stopped an ordered projection subscription. */
export const SubscriptionStoppedEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("subscription.stopped"),
	payload: Schema.Struct({}),
	subscription_id: Identifier,
});

export type SubscriptionStoppedEnvelope = typeof SubscriptionStoppedEnvelope.Type;

/** Provides ephemeral thread-list state; reconnecting clients request a fresh snapshot. */
export const ThreadListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.list.snapshot"),
	payload: Schema.Struct({
		threads: Schema.Array(ThreadListItem),
	}),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type ThreadListSnapshotEnvelope = typeof ThreadListSnapshotEnvelope.Type;

/** Delivers one connection-local thread-list upsert after a subscription snapshot. */
export const ThreadListUpsertEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.list.upsert"),
	payload: ThreadListItem,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type ThreadListUpsertEnvelope = typeof ThreadListUpsertEnvelope.Type;

/** Removes one erased thread from an ordered connection-local thread list. */
export const ThreadListRemoveEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.list.remove"),
	payload: Schema.Struct({ thread_id: Identifier }),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type ThreadListRemoveEnvelope = typeof ThreadListRemoveEnvelope.Type;

/** Provides the current Forge-owned project catalog for one ordered subscription. */
export const ProjectListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	kind: Schema.Literal("project.list.snapshot"),
	payload: ProjectCatalogSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ProjectListSnapshotEnvelope = typeof ProjectListSnapshotEnvelope.Type;

/** Replaces the Forge-owned project catalog after an attach or detach mutation. */
export const ProjectListUpdatedEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	kind: Schema.Literal("project.list.updated"),
	payload: ProjectCatalogSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ProjectListUpdatedEnvelope = typeof ProjectListUpdatedEnvelope.Type;

/** Provides the initial graph projection for one ordered subscription. */
export const OrchestrationGraphSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("orchestration.graph.snapshot"),
	payload: Schema.Struct({ graph: OrchestrationGraph }),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type OrchestrationGraphSnapshotEnvelope = typeof OrchestrationGraphSnapshotEnvelope.Type;

/** Delivers an ordered replacement patch for one graph subscription. */
export const OrchestrationGraphPatchEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("orchestration.graph.patch"),
	payload: Schema.Struct({ graph: OrchestrationGraph }),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type OrchestrationGraphPatchEnvelope = typeof OrchestrationGraphPatchEnvelope.Type;

export const ThreadTranscriptSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.transcript.snapshot"),
	payload: ThreadTranscriptSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ThreadTranscriptSnapshotEnvelope = typeof ThreadTranscriptSnapshotEnvelope.Type;

export const ThreadTranscriptAppendEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.transcript.append"),
	payload: Schema.Struct({ entries: Schema.Array(TranscriptEntry) }),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ThreadTranscriptAppendEnvelope = typeof ThreadTranscriptAppendEnvelope.Type;

export const ConversationSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("conversation.snapshot"),
	payload: ConversationSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ConversationSnapshotEnvelope = typeof ConversationSnapshotEnvelope.Type;

export const ConversationPatchEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("conversation.patch"),
	payload: ConversationPatchBatch,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ConversationPatchEnvelope = typeof ConversationPatchEnvelope.Type;

export const OrchestrationGroupListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("orchestration.group.list.snapshot"),
	payload: OrchestrationGroupListSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type OrchestrationGroupListSnapshotEnvelope =
	typeof OrchestrationGroupListSnapshotEnvelope.Type;

export const OrchestrationGroupListPatchEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("orchestration.group.list.patch"),
	payload: OrchestrationGroupListSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type OrchestrationGroupListPatchEnvelope = typeof OrchestrationGroupListPatchEnvelope.Type;

export const ThreadSessionSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.session.snapshot"),
	payload: ThreadSessionSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ThreadSessionSnapshotEnvelope = typeof ThreadSessionSnapshotEnvelope.Type;
export const SurfaceListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("surface.list.snapshot"),
	payload: SurfaceSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type SurfaceListSnapshotEnvelope = typeof SurfaceListSnapshotEnvelope.Type;
export const SurfaceUsageAggregateSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("surface.usage.aggregate.snapshot"),
	payload: SurfaceUsageAggregateSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type SurfaceUsageAggregateSnapshotEnvelope =
	typeof SurfaceUsageAggregateSnapshotEnvelope.Type;
export const WorkspaceConflictListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("workspace.conflict.list.snapshot"),
	payload: WorkspaceConflictListQueryResult,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type WorkspaceConflictListSnapshotEnvelope =
	typeof WorkspaceConflictListSnapshotEnvelope.Type;

/** Acknowledges the highest contiguous journal position and durable event cursors. */
export const AckEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("ack"),
	payload: Schema.Struct({
		event_cursors: Schema.Array(StreamCursor),
		journal_sequence: JournalSequence,
	}),
});

export type AckEnvelope = typeof AckEnvelope.Type;

/** Requests durable replay after a global journal cursor with optional stream checks. */
export const ReplayEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("replay"),
	payload: Schema.Struct({
		after_journal_sequence: JournalSequence,
		event_cursors: Schema.optional(Schema.Array(StreamCursor)),
	}),
});

export type ReplayEnvelope = typeof ReplayEnvelope.Type;

/** Marks a replay boundary and reports the current global and stream positions. */
export const ReplayCompleteEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("replay.complete"),
	payload: Schema.Struct({
		current_event_cursors: Schema.Array(StreamCursor),
		journal_sequence: JournalSequence,
	}),
});

export type ReplayCompleteEnvelope = typeof ReplayCompleteEnvelope.Type;

/** Checks client liveness from the backend side of an idle control connection. */
export const HeartbeatPingEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	kind: Schema.Literal("heartbeat.ping"),
	payload: Schema.Struct({
		nonce: Identifier,
	}),
});

export type HeartbeatPingEnvelope = typeof HeartbeatPingEnvelope.Type;

/** Responds from the frontend using the backend ping identifier and nonce. */
export const HeartbeatPongEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("heartbeat.pong"),
	payload: Schema.Struct({
		nonce: Identifier,
	}),
});

export type HeartbeatPongEnvelope = typeof HeartbeatPongEnvelope.Type;

/** Requests the policy-aware built-in tool registry and renderer-safe usage snapshot. */
export const ArtisanToolRegistryListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("artisan.tool.registry.list.query"),
	payload: ArtisanToolRegistryListQuery,
});

export type ArtisanToolRegistryListQueryEnvelope = typeof ArtisanToolRegistryListQueryEnvelope.Type;

/** Returns available built-in tools, their policy state, and bounded usage totals. */
export const ArtisanToolRegistryListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("artisan.tool.registry.list.query.result"),
	payload: ArtisanToolRegistryListQueryResult,
});

export type ArtisanToolRegistryListQueryResultEnvelope =
	typeof ArtisanToolRegistryListQueryResultEnvelope.Type;

/** Starts one durable policy-routed Artisan-owned tool invocation. */
export const ArtisanToolExecuteEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("artisan.tool.execute"),
	payload: ArtisanToolExecutionRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type ArtisanToolExecuteEnvelope = typeof ArtisanToolExecuteEnvelope.Type;

/** Resolves one exact pending tool approval through the canonical policy path. */
export const ArtisanApprovalResolveEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	kind: Schema.Literal("artisan.approval.resolve"),
	payload: ArtisanApprovalResolveRequest,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type ArtisanApprovalResolveEnvelope = typeof ArtisanApprovalResolveEnvelope.Type;

/** Requests bounded, cursor-aware invocation history for one visible thread. */
export const ArtisanToolInvocationListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("artisan.tool.invocation.list.query"),
	payload: ArtisanToolInvocationListQuery,
});

export type ArtisanToolInvocationListQueryEnvelope =
	typeof ArtisanToolInvocationListQueryEnvelope.Type;

/** Returns invocation history and its durable ledger position. */
export const ArtisanToolInvocationListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("artisan.tool.invocation.list.query.result"),
	payload: ArtisanToolInvocationListQueryResult,
});

export type ArtisanToolInvocationListQueryResultEnvelope =
	typeof ArtisanToolInvocationListQueryResultEnvelope.Type;

/** Requests pending or resolved approvals belonging to one visible thread. */
export const ArtisanApprovalListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("artisan.approval.list.query"),
	payload: ArtisanApprovalListQuery,
});

export type ArtisanApprovalListQueryEnvelope = typeof ArtisanApprovalListQueryEnvelope.Type;

/** Returns durable approvals and the ledger position represented by the response. */
export const ArtisanApprovalListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("artisan.approval.list.query.result"),
	payload: ArtisanApprovalListQueryResult,
});

export type ArtisanApprovalListQueryResultEnvelope =
	typeof ArtisanApprovalListQueryResultEnvelope.Type;

/** Requests root-confined workspace path metadata without exposing filesystem access to renderers. */
export const WorkspaceFileDiscoveryQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.file.discovery.query"),
	payload: WorkspaceFileDiscoveryQuery,
});

export type WorkspaceFileDiscoveryQueryEnvelope = typeof WorkspaceFileDiscoveryQueryEnvelope.Type;

/** Returns one bounded page of content-free workspace path metadata. */
export const WorkspaceFileDiscoveryQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.file.discovery.query.result"),
	payload: WorkspaceFileDiscoveryQueryResult,
});

export type WorkspaceFileDiscoveryQueryResultEnvelope =
	typeof WorkspaceFileDiscoveryQueryResultEnvelope.Type;

/** Requests the truthful backend-owned language and diagnostics capability projection. */
export const WorkspaceLanguageCapabilitiesQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.language.capabilities.query"),
	payload: WorkspaceLanguageCapabilitiesQuery,
});

export type WorkspaceLanguageCapabilitiesQueryEnvelope =
	typeof WorkspaceLanguageCapabilitiesQueryEnvelope.Type;

/** Returns language feature availability without implying local renderer capabilities. */
export const WorkspaceLanguageCapabilitiesQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.language.capabilities.query.result"),
	payload: WorkspaceLanguageCapabilitiesQueryResult,
});

export type WorkspaceLanguageCapabilitiesQueryResultEnvelope =
	typeof WorkspaceLanguageCapabilitiesQueryResultEnvelope.Type;

/** Decodes every client-to-backend frame accepted on the control channel. */
export const InboundControlEnvelope = Schema.Union([
	HelloEnvelope,
	CommandEnvelope,
	ThreadCreateEnvelope,
	ThreadListQueryEnvelope,
	ProjectDirectoryListQueryEnvelope,
	ProjectDirectorySelectEnvelope,
	ProjectListQueryEnvelope,
	ProjectDetachEnvelope,
	RuntimeCatalogQueryEnvelope,
	ThreadRetentionQueryEnvelope,
	ThreadRetentionUpdateEnvelope,
	WorkspaceFileReadQueryEnvelope,
	WorkspaceFileReplaceEnvelope,
	WorkspaceChangeReviewEnvelope,
	WorkspaceChangeRollbackEnvelope,
	WorkspaceChangeListQueryEnvelope,
	WorkspaceConflictListQueryEnvelope,
	WorkspaceChangeDiffQueryEnvelope,
	GitWorkspaceQueryEnvelope,
	GitDiffQueryEnvelope,
	GitIndexStageRequestEnvelope,
	GitIndexUnstageRequestEnvelope,
	GitMutationResolveEnvelope,
	GlobalGuidanceQueryEnvelope,
	GlobalGuidanceUpdateEnvelope,
	GlobalGuidanceSelectionEnvelope,
	GlobalGuidanceDriftResolutionEnvelope,
	GlobalGuidanceRetryEnvelope,
	ModelBehaviourQueryEnvelope,
	ModelBehaviourUpdateEnvelope,
	ModelBehaviourDriftResolutionEnvelope,
	ModelBehaviourRetryEnvelope,
	RoutineRegistryQueryEnvelope,
	RoutineDetailQueryEnvelope,
	RoutineInstallPreviewEnvelope,
	RoutineInstallRequestEnvelope,
	RoutineApprovalDecisionEnvelope,
	RoutineEnableEnvelope,
	RoutineDisableEnvelope,
	RoutineRemoveEnvelope,
	RoutineSyncEnvelope,
	RoutineDriftResolutionEnvelope,
	RoutineDriftOverwriteRequestEnvelope,
	RoutineDriftOverwriteDecisionEnvelope,
	RoutineInvokeEnvelope,
	RoutineRollbackEnvelope,
	NpxSkillsDiscoverEnvelope,
	NpxSkillsImportEnvelope,
	CapabilityRegistryQueryEnvelope,
	CapabilityDetailQueryEnvelope,
	CapabilityConnectPreviewEnvelope,
	CapabilityConnectRequestEnvelope,
	CapabilityApprovalDecisionEnvelope,
	CapabilityStartEnvelope,
	CapabilityReconnectEnvelope,
	CapabilityHealthEnvelope,
	CapabilityDisconnectEnvelope,
	CapabilityRestartEnvelope,
	CapabilityUninstallEnvelope,
	CapabilityEnableEnvelope,
	CapabilityDisableEnvelope,
	CapabilityRemoveEnvelope,
	CapabilitySyncEnvelope,
	CapabilityDriftResolutionEnvelope,
	CapabilityDriftOverwriteRequestEnvelope,
	CapabilityDriftOverwriteDecisionEnvelope,
	CapabilityInvocationApprovalRequestEnvelope,
	CapabilityInvocationApprovalDecisionEnvelope,
	CapabilityInvokeEnvelope,
	CapabilityOAuthBeginEnvelope,
	CapabilityOAuthCompleteEnvelope,
	CapabilityOAuthRefreshEnvelope,
	CapabilityOAuthRevokeEnvelope,
	CapabilityOAuthTokenStatusEnvelope,
	ThreadWorkQueryEnvelope,
	TerminalListQueryEnvelope,
	OrchestrationGraphQueryEnvelope,
	ThreadTranscriptQueryEnvelope,
	ConversationQueryEnvelope,
	MessageImageAttachmentQueryEnvelope,
	OrchestrationGroupListQueryEnvelope,
	ArtisanToolRegistryListQueryEnvelope,
	ArtisanToolExecuteEnvelope,
	ArtisanApprovalResolveEnvelope,
	ArtisanToolInvocationListQueryEnvelope,
	ArtisanApprovalListQueryEnvelope,
	WorkspaceFileDiscoveryQueryEnvelope,
	WorkspaceLanguageCapabilitiesQueryEnvelope,
	PreviewTargetListQueryEnvelope,
	PreviewTargetGetQueryEnvelope,
	PreviewTargetRegisterEnvelope,
	PreviewTargetProbeEnvelope,
	PreviewTargetStateEnvelope,
	PreviewTargetRemoveEnvelope,
	RichLinkResolveQueryEnvelope,
	PreviewAssetMetadataQueryEnvelope,
	PreviewBrowserLaunchEnvelope,
	PreviewInspectionSessionOpenEnvelope,
	PreviewInspectionEnvelope,
	PreviewInspectionSessionCloseEnvelope,
	ThreadSessionQueryEnvelope,
	SurfaceListQueryEnvelope,
	SurfaceUsageAggregateQueryEnvelope,
	SurfaceUsageDailyQueryEnvelope,
	SubscribeEnvelope,
	UnsubscribeEnvelope,
	AckEnvelope,
	ReplayEnvelope,
	HeartbeatPongEnvelope,
]);

export type InboundControlEnvelope = typeof InboundControlEnvelope.Type;

/** Encodes every backend-to-client frame emitted on the control channel. */
export const OutboundControlEnvelope = Schema.Union([
	WelcomeEnvelope,
	PreNegotiationProtocolErrorEnvelope,
	CommandReceiptEnvelope,
	EventEnvelope,
	ProtocolErrorEnvelope,
	ThreadCreateResultEnvelope,
	ThreadListQueryResultEnvelope,
	ProjectDirectoryListQueryResultEnvelope,
	ProjectDirectorySelectResultEnvelope,
	ProjectListQueryResultEnvelope,
	ProjectDetachResultEnvelope,
	ProjectListSnapshotEnvelope,
	ProjectListUpdatedEnvelope,
	RuntimeCatalogQueryResultEnvelope,
	ThreadRetentionQueryResultEnvelope,
	WorkspaceFileReadQueryResultEnvelope,
	WorkspaceChangeListQueryResultEnvelope,
	WorkspaceConflictListQueryResultEnvelope,
	WorkspaceChangeDiffQueryResultEnvelope,
	GitWorkspaceQueryResultEnvelope,
	GitDiffQueryResultEnvelope,
	GlobalGuidanceQueryResultEnvelope,
	ModelBehaviourQueryResultEnvelope,
	RoutineRegistryQueryResultEnvelope,
	RoutineDetailQueryResultEnvelope,
	RoutineInstallPreviewResultEnvelope,
	RoutineInvokeResultEnvelope,
	NpxSkillsDiscoverResultEnvelope,
	CapabilityRegistryQueryResultEnvelope,
	CapabilityDetailQueryResultEnvelope,
	CapabilityConnectPreviewResultEnvelope,
	CapabilityInvokeResultEnvelope,
	CapabilityOAuthTokenStatusResultEnvelope,
	CapabilityOAuthBeginResultEnvelope,
	ThreadWorkQueryResultEnvelope,
	TerminalListQueryResultEnvelope,
	OrchestrationGraphQueryResultEnvelope,
	ThreadTranscriptQueryResultEnvelope,
	ConversationQueryResultEnvelope,
	MessageImageAttachmentQueryResultEnvelope,
	OrchestrationGroupListQueryResultEnvelope,
	ArtisanToolRegistryListQueryResultEnvelope,
	ArtisanToolInvocationListQueryResultEnvelope,
	ArtisanApprovalListQueryResultEnvelope,
	WorkspaceFileDiscoveryQueryResultEnvelope,
	WorkspaceLanguageCapabilitiesQueryResultEnvelope,
	PreviewTargetListQueryResultEnvelope,
	PreviewTargetGetQueryResultEnvelope,
	PreviewTargetMutationResultEnvelope,
	RichLinkResolveQueryResultEnvelope,
	PreviewAssetMetadataQueryResultEnvelope,
	PreviewBrowserLaunchResultEnvelope,
	PreviewInspectionSessionOpenResultEnvelope,
	PreviewInspectionResultEnvelope,
	PreviewInspectionSessionCloseResultEnvelope,
	ThreadSessionQueryResultEnvelope,
	SurfaceListQueryResultEnvelope,
	SurfaceUsageAggregateQueryResultEnvelope,
	SurfaceUsageDailyQueryResultEnvelope,
	SubscriptionStartedEnvelope,
	SubscriptionStoppedEnvelope,
	ThreadListSnapshotEnvelope,
	ThreadListUpsertEnvelope,
	ThreadListRemoveEnvelope,
	OrchestrationGraphSnapshotEnvelope,
	OrchestrationGraphPatchEnvelope,
	ThreadTranscriptSnapshotEnvelope,
	ThreadTranscriptAppendEnvelope,
	ConversationSnapshotEnvelope,
	ConversationPatchEnvelope,
	OrchestrationGroupListSnapshotEnvelope,
	OrchestrationGroupListPatchEnvelope,
	ThreadSessionSnapshotEnvelope,
	SurfaceListSnapshotEnvelope,
	SurfaceUsageAggregateSnapshotEnvelope,
	WorkspaceConflictListSnapshotEnvelope,
	ReplayCompleteEnvelope,
	HeartbeatPingEnvelope,
]);

export type OutboundControlEnvelope = typeof OutboundControlEnvelope.Type;

/** Represents every validated V1 control-channel frame in either direction. */
export const ControlEnvelope = Schema.Union([InboundControlEnvelope, OutboundControlEnvelope]);

export type ControlEnvelope = typeof ControlEnvelope.Type;

/** Preserves the legacy name for backend-originated control frames. */
export const OutboundEnvelope = OutboundControlEnvelope;

export type OutboundEnvelope = typeof OutboundEnvelope.Type;

/** Preserves the legacy name for control frames in either direction. */
export const WireEnvelope = ControlEnvelope;

export type WireEnvelope = typeof WireEnvelope.Type;
