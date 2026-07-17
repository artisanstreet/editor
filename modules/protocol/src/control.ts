import { Schema } from "effect";

import {
	WorkspaceChangeListQuery,
	WorkspaceChangeListQueryResult,
	WorkspaceChangeDiffQuery,
	WorkspaceChangeDiffQueryResult,
	WorkspaceChangeReviewRequest,
	WorkspaceChangeRollbackRequest,
	WorkspaceChangeUpdatedEvent,
	WorkspaceReplaceApprovalQuery,
	WorkspaceReplaceApprovalQueryResult,
	WorkspaceReplaceApprovalResponseRequest,
	WorkspaceReplaceApprovalUpdatedEvent,
	WorkspaceFileReadQuery,
	WorkspaceFileReadQueryResult,
	WorkspaceFileReplaceRequest,
} from "./workspace-changes";
import { CapabilityInvocationUpdatedEvent, EngineNativeActionObservedEvent } from "./capability";
import {
	DecideApprovalRequest,
	DecideApprovalResult,
	ToolApprovalQuery,
	ToolApprovalQueryResult,
	ToolApprovalUpdatedEvent,
	ToolInvocationQuery,
	ToolInvocationQueryResult,
	ToolInvocationUpdatedEvent,
} from "./tool-control";
import {
	WorkspaceGitCheckoutApprovalQuery,
	WorkspaceGitCheckoutApprovalQueryResult,
	WorkspaceGitCheckoutApprovalResponseRequest,
	WorkspaceGitCheckoutApprovalUpdatedEvent,
	WorkspaceGitCheckoutRequest,
	WorkspaceGitSessionQuery,
	WorkspaceGitSessionQueryResult,
	WorkspaceGitSessionRefreshRequest,
	WorkspaceGitSessionUpdatedEvent,
} from "./git-session";
import {
	WorkspaceGitMutationApprovalQuery,
	WorkspaceGitMutationApprovalQueryResult,
	WorkspaceGitMutationApprovalResponseRequest,
	WorkspaceGitMutationApprovalUpdatedEvent,
	WorkspaceGitMutationRequest,
} from "./git-mutation";
import {
	HostedProjectCloneApprovalQuery,
	HostedProjectCloneApprovalQueryResult,
	HostedProjectCloneApprovalResponseRequest,
	HostedProjectCloneApprovalUpdatedEvent,
	HostedProjectCloneRequest,
} from "./hosted-project";
import {
	HostedGitCheckFailureDetailQuery,
	HostedGitCheckFailureDetailQueryResult,
	HostedGitMutationApprovalQuery,
	HostedGitMutationApprovalQueryResult,
	HostedGitMutationApprovalResponseRequest,
	HostedGitMutationApprovalUpdatedEvent,
	HostedGitMutationCommandRequest,
	HostedGitSnapshotQuery,
	HostedGitSnapshotQueryResult,
	HostedGitSnapshotRefreshRequest,
	HostedGitSnapshotUpdatedEvent,
} from "./hosted-git";
import {
	ExternalWaitCancelRequest,
	ExternalWaitManualResumeRequest,
	ExternalWaitQuery,
	ExternalWaitQueryResult,
	ExternalWaitRequest,
	ExternalWaitUpdatedEvent,
} from "./external-wait";
import {
	WorkspaceGitFetchCompletedEvent,
	WorkspaceGitFetchPolicyUpdate,
	WorkspaceGitFetchPolicyUpdatedEvent,
	WorkspaceGitFetchQuery,
	WorkspaceGitFetchQueryResult,
	WorkspaceGitFetchRequest,
	WorkspaceGitFetchRequestedEvent,
} from "./local-git-fetch";
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
	PreviewBrowserCommand,
	PreviewBrowserLifecycleEvent,
	PreviewBrowserLifecycleQuery,
	PreviewBrowserLifecycleQueryResult,
	PreviewTargetCommand,
	PreviewTargetUpdatedEvent,
	PreviewTargetsQuery,
	PreviewTargetsQueryResult,
} from "./preview";
import { RichLinkMetadataQuery, RichLinkMetadataQueryResult } from "./rich-link";

export * from "./thread";
export * from "./guidance";
export * from "./model-behaviour";
export * from "./workspace-changes";
export * from "./git-session";
export * from "./hosted-git";
export * from "./preview";
export * from "./rich-link";

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
	type: Schema.Literal("thread.send_message"),
	engine_id: Identifier,
	mentioned_projects: Schema.optional(Schema.Array(ProjectRef)),
	text: Schema.NonEmptyString,
	working_directory: Schema.NonEmptyString,
});

export type ThreadSendMessageCommand = typeof ThreadSendMessageCommand.Type;

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
	assignments: Schema.Array(AssignmentSpec).check(Schema.isMinLength(2)),
	edges: Schema.optional(Schema.Array(GraphEdgeSpec)),
	joins: Schema.optional(Schema.Array(JoinSpec)),
	name_bank: Schema.optional(Schema.NonEmptyArray(graph_visible_name)),
	max_concurrency: Schema.optional(PositiveInt),
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
	TerminalOpenCommand,
	TerminalWriteCommand,
	TerminalResizeCommand,
	TerminalClearCommand,
	TerminalKillCommand,
	TerminalCloseCommand,
	TerminalRestartCommand,
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
	PreviewTargetCommand,
	PreviewBrowserCommand,
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
	type: Schema.Literal("thread.message_queued"),
	message_id: Identifier,
	mentioned_projects: Schema.optional(Schema.Array(ProjectRef)),
	reason: Schema.Literals(["no_active_run", "steering_rejected", "unsupported"]),
	text: Schema.NonEmptyString,
	working_directory: Schema.NonEmptyString,
});

/** Records user text accepted as a steering request for a live run. */
export const ThreadMessageSteeringEvent = Schema.Struct({
	type: Schema.Literal("thread.message_steering"),
	message_id: Identifier,
	mentioned_projects: Schema.optional(Schema.Array(ProjectRef)),
	text: Schema.NonEmptyString,
	working_directory: Schema.NonEmptyString,
});

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

/** Projects provider-neutral token totals without provider pricing metadata. */
export const RunUsage = Schema.Struct({
	input_tokens: Schema.Int.pipe(
		Schema.check(Schema.isGreaterThanOrEqualTo(0)),
		Schema.check(Schema.isLessThanOrEqualTo(Number.MAX_SAFE_INTEGER)),
	),
	output_tokens: Schema.Int.pipe(
		Schema.check(Schema.isGreaterThanOrEqualTo(0)),
		Schema.check(Schema.isLessThanOrEqualTo(Number.MAX_SAFE_INTEGER)),
	),
});

export type RunUsage = typeof RunUsage.Type;

/** Records a durable provider-neutral usage projection for one run. */
export const RunUsageUpdatedEvent = Schema.Struct({
	type: Schema.Literal("run.usage.updated"),
	usage: RunUsage,
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
	usage: Schema.optional(RunUsage),
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
	WorkspaceChangeUpdatedEvent,
	WorkspaceReplaceApprovalUpdatedEvent,
	WorkspaceGitSessionUpdatedEvent,
	WorkspaceGitCheckoutApprovalUpdatedEvent,
	WorkspaceGitMutationApprovalUpdatedEvent,
	WorkspaceGitFetchPolicyUpdatedEvent,
	WorkspaceGitFetchRequestedEvent,
	WorkspaceGitFetchCompletedEvent,
	HostedProjectCloneApprovalUpdatedEvent,
	HostedGitMutationApprovalUpdatedEvent,
	HostedGitSnapshotUpdatedEvent,
	ExternalWaitUpdatedEvent,
	ThreadMessageQueuedEvent,
	ThreadMessageSteeringEvent,
	RunLifecycleEvent,
	RunUsageUpdatedEvent,
	AssistantMessageCompletedEvent,
	ApprovalInteractionEvent,
	QuestionInteractionEvent,
	FilesystemMutationEvent,
	ProcessOwnershipEvent,
	GitWorkspaceObservedEvent,
	TerminalLifecycleEvent,
	OrchestrationGraphLifecycleEvent,
	AssignmentHeartbeatEvent,
	AgentInstanceRenamedEvent,
	AssignmentControlEvent,
	ArtifactRecordedEvent,
	PreviewTargetUpdatedEvent,
	PreviewBrowserLifecycleEvent,
	CapabilityInvocationUpdatedEvent,
	EngineNativeActionObservedEvent,
	ToolInvocationUpdatedEvent,
	ToolApprovalUpdatedEvent,
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

/** Requests one workspace replacement approval and its bounded private unified diff. */
export const WorkspaceReplaceApprovalQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.replace.approval.query"),
	payload: WorkspaceReplaceApprovalQuery,
});

export type WorkspaceReplaceApprovalQueryEnvelope =
	typeof WorkspaceReplaceApprovalQueryEnvelope.Type;

/** Returns one correlated workspace replacement approval and its private unified diff. */
export const WorkspaceReplaceApprovalQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.replace.approval.query.result"),
	payload: WorkspaceReplaceApprovalQueryResult,
});

export type WorkspaceReplaceApprovalQueryResultEnvelope =
	typeof WorkspaceReplaceApprovalQueryResultEnvelope.Type;

/** Records an explicit frontend approval or denial decision for one workspace replacement. */
export const WorkspaceReplaceApprovalRespondEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.replace.approval.respond"),
	payload: WorkspaceReplaceApprovalResponseRequest,
	thread_id: Identifier,
});

export type WorkspaceReplaceApprovalRespondEnvelope =
	typeof WorkspaceReplaceApprovalRespondEnvelope.Type;

/** Requests the current Git session projection for one workspace. */
export const WorkspaceGitSessionQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.session.query"),
	payload: WorkspaceGitSessionQuery,
});

export type WorkspaceGitSessionQueryEnvelope = typeof WorkspaceGitSessionQueryEnvelope.Type;

/** Returns a correlated optional Git session projection. */
export const WorkspaceGitSessionQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.git.session.query.result"),
	payload: WorkspaceGitSessionQueryResult,
});

export type WorkspaceGitSessionQueryResultEnvelope =
	typeof WorkspaceGitSessionQueryResultEnvelope.Type;

/** Requests a fresh Git session observation for one workspace. */
export const WorkspaceGitSessionRefreshEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.session.refresh"),
	payload: WorkspaceGitSessionRefreshRequest,
	thread_id: Identifier,
});

export type WorkspaceGitSessionRefreshEnvelope = typeof WorkspaceGitSessionRefreshEnvelope.Type;

/** Requests the global local Git fetch policy and workspace attempt states. */
export const WorkspaceGitFetchQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.fetch.query"),
	payload: WorkspaceGitFetchQuery,
});

export type WorkspaceGitFetchQueryEnvelope = typeof WorkspaceGitFetchQueryEnvelope.Type;

/** Returns a correlated global fetch policy and bounded workspace states. */
export const WorkspaceGitFetchQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.git.fetch.query.result"),
	payload: WorkspaceGitFetchQueryResult,
});

export type WorkspaceGitFetchQueryResultEnvelope = typeof WorkspaceGitFetchQueryResultEnvelope.Type;

/** Updates the global automatic local Git fetch policy. */
export const WorkspaceGitFetchPolicyUpdateEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.fetch.policy.update"),
	payload: WorkspaceGitFetchPolicyUpdate,
});

export type WorkspaceGitFetchPolicyUpdateEnvelope =
	typeof WorkspaceGitFetchPolicyUpdateEnvelope.Type;

/** Requests a manual local Git fetch for one thread-owned workspace. */
export const WorkspaceGitFetchRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.fetch.request"),
	payload: WorkspaceGitFetchRequest,
	thread_id: Identifier,
});

export type WorkspaceGitFetchRequestEnvelope = typeof WorkspaceGitFetchRequestEnvelope.Type;

/** Requests the latest durable hosted review and CI projection for one workspace. */
export const HostedGitSnapshotQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.git.snapshot.query"),
	payload: HostedGitSnapshotQuery,
});

export type HostedGitSnapshotQueryEnvelope = typeof HostedGitSnapshotQueryEnvelope.Type;

/** Requests fresh bounded detail for one failed hosted check. */
export const HostedGitCheckFailureDetailQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.git.check_failure_detail.query"),
	payload: HostedGitCheckFailureDetailQuery,
});

export type HostedGitCheckFailureDetailQueryEnvelope =
	typeof HostedGitCheckFailureDetailQueryEnvelope.Type;

/** Returns one correlated bounded hosted check failure detail. */
export const HostedGitCheckFailureDetailQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("hosted.git.check_failure_detail.query.result"),
	payload: HostedGitCheckFailureDetailQueryResult,
});

export type HostedGitCheckFailureDetailQueryResultEnvelope =
	typeof HostedGitCheckFailureDetailQueryResultEnvelope.Type;

/** Returns one correlated optional hosted review and CI projection. */
export const HostedGitSnapshotQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("hosted.git.snapshot.query.result"),
	payload: HostedGitSnapshotQueryResult,
});

export type HostedGitSnapshotQueryResultEnvelope = typeof HostedGitSnapshotQueryResultEnvelope.Type;

/** Requests a fresh exact-head hosted review and CI observation. */
export const HostedGitSnapshotRefreshEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.git.snapshot.refresh"),
	payload: HostedGitSnapshotRefreshRequest,
	thread_id: Identifier,
});

export type HostedGitSnapshotRefreshEnvelope = typeof HostedGitSnapshotRefreshEnvelope.Type;

/** Requests registration of one durable provider-neutral external wait. */
export const ExternalWaitRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("external_wait.request"),
	payload: ExternalWaitRequest,
	thread_id: Identifier,
});

export type ExternalWaitRequestEnvelope = typeof ExternalWaitRequestEnvelope.Type;

/** Requests cancellation of one durable external wait. */
export const ExternalWaitCancelEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("external_wait.cancel"),
	payload: ExternalWaitCancelRequest,
	thread_id: Identifier,
});

export type ExternalWaitCancelEnvelope = typeof ExternalWaitCancelEnvelope.Type;

/** Requests a user-triggered resume of one durable external wait. */
export const ExternalWaitManualResumeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("external_wait.manual_resume"),
	payload: ExternalWaitManualResumeRequest,
	thread_id: Identifier,
});

export type ExternalWaitManualResumeEnvelope = typeof ExternalWaitManualResumeEnvelope.Type;

/** Queries the durable external wait for one thread. */
export const ExternalWaitQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("external_wait.query"),
	payload: ExternalWaitQuery,
});

export type ExternalWaitQueryEnvelope = typeof ExternalWaitQueryEnvelope.Type;

/** Returns the optional durable external wait for one correlated thread query. */
export const ExternalWaitQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("external_wait.query.result"),
	payload: ExternalWaitQueryResult,
});

export type ExternalWaitQueryResultEnvelope = typeof ExternalWaitQueryResultEnvelope.Type;

/** Queries the current local preview targets in one workspace and project. */
export const PreviewTargetsQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.targets.query"),
	payload: PreviewTargetsQuery,
});

export type PreviewTargetsQueryEnvelope = typeof PreviewTargetsQueryEnvelope.Type;

/** Returns the current preview targets for a correlated query. */
export const PreviewTargetsQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.targets.query.result"),
	payload: PreviewTargetsQueryResult,
});

export type PreviewTargetsQueryResultEnvelope = typeof PreviewTargetsQueryResultEnvelope.Type;

/** Queries external-browser launches and inspection sessions in one exact scope. */
export const PreviewBrowserLifecycleQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("preview.browser.lifecycle.query"),
	payload: PreviewBrowserLifecycleQuery,
});

export type PreviewBrowserLifecycleQueryEnvelope = typeof PreviewBrowserLifecycleQueryEnvelope.Type;

/** Returns external-browser launches and inspection sessions for a correlated query. */
export const PreviewBrowserLifecycleQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("preview.browser.lifecycle.query.result"),
	payload: PreviewBrowserLifecycleQueryResult,
});

export type PreviewBrowserLifecycleQueryResultEnvelope =
	typeof PreviewBrowserLifecycleQueryResultEnvelope.Type;

/** Queries bounded metadata for one external HTTP(S) URL. */
export const RichLinkMetadataQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("rich-link.metadata.query"),
	payload: RichLinkMetadataQuery,
});

export type RichLinkMetadataQueryEnvelope = typeof RichLinkMetadataQueryEnvelope.Type;

/** Returns normalized rich-link metadata for a correlated query. */
export const RichLinkMetadataQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("rich-link.metadata.query.result"),
	payload: RichLinkMetadataQueryResult,
});

export type RichLinkMetadataQueryResultEnvelope = typeof RichLinkMetadataQueryResultEnvelope.Type;

/** Requests a guarded checkout of a workspace branch. */
export const WorkspaceGitCheckoutRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.checkout.request"),
	payload: WorkspaceGitCheckoutRequest,
	thread_id: Identifier,
});

export type WorkspaceGitCheckoutRequestEnvelope = typeof WorkspaceGitCheckoutRequestEnvelope.Type;

/** Requests one source-free checkout approval by durable identity. */
export const WorkspaceGitCheckoutApprovalQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.checkout.approval.query"),
	payload: WorkspaceGitCheckoutApprovalQuery,
});

export type WorkspaceGitCheckoutApprovalQueryEnvelope =
	typeof WorkspaceGitCheckoutApprovalQueryEnvelope.Type;

/** Returns a correlated source-free checkout approval projection. */
export const WorkspaceGitCheckoutApprovalQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.git.checkout.approval.query.result"),
	payload: WorkspaceGitCheckoutApprovalQueryResult,
});

export type WorkspaceGitCheckoutApprovalQueryResultEnvelope =
	typeof WorkspaceGitCheckoutApprovalQueryResultEnvelope.Type;

/** Records an explicit approval or denial for one checkout request. */
export const WorkspaceGitCheckoutApprovalRespondEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.checkout.approval.respond"),
	payload: WorkspaceGitCheckoutApprovalResponseRequest,
	thread_id: Identifier,
});

export type WorkspaceGitCheckoutApprovalRespondEnvelope =
	typeof WorkspaceGitCheckoutApprovalRespondEnvelope.Type;

/** Requests one guarded Git mutation against an observed session version. */
export const WorkspaceGitMutationRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.mutation.request"),
	payload: WorkspaceGitMutationRequest,
	thread_id: Identifier,
});

export type WorkspaceGitMutationRequestEnvelope = typeof WorkspaceGitMutationRequestEnvelope.Type;

/** Requests one source-free Git mutation approval by durable identity. */
export const WorkspaceGitMutationApprovalQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.mutation.approval.query"),
	payload: WorkspaceGitMutationApprovalQuery,
});

export type WorkspaceGitMutationApprovalQueryEnvelope =
	typeof WorkspaceGitMutationApprovalQueryEnvelope.Type;

/** Returns a correlated public Git mutation approval projection. */
export const WorkspaceGitMutationApprovalQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("workspace.git.mutation.approval.query.result"),
	payload: WorkspaceGitMutationApprovalQueryResult,
});

export type WorkspaceGitMutationApprovalQueryResultEnvelope =
	typeof WorkspaceGitMutationApprovalQueryResultEnvelope.Type;

/** Records an explicit approval or denial for one Git mutation request. */
export const WorkspaceGitMutationApprovalRespondEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("workspace.git.mutation.approval.respond"),
	payload: WorkspaceGitMutationApprovalResponseRequest,
	thread_id: Identifier,
});

export type WorkspaceGitMutationApprovalRespondEnvelope =
	typeof WorkspaceGitMutationApprovalRespondEnvelope.Type;

/** Requests preparation of one hosted repository clone. */
export const HostedProjectCloneRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.project.clone.request"),
	payload: HostedProjectCloneRequest,
	thread_id: Identifier,
});

export type HostedProjectCloneRequestEnvelope = typeof HostedProjectCloneRequestEnvelope.Type;

/** Requests one source-free hosted clone approval by durable identity. */
export const HostedProjectCloneApprovalQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.project.clone.approval.query"),
	payload: HostedProjectCloneApprovalQuery,
});

export type HostedProjectCloneApprovalQueryEnvelope =
	typeof HostedProjectCloneApprovalQueryEnvelope.Type;

/** Returns a correlated source-free hosted clone approval projection. */
export const HostedProjectCloneApprovalQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("hosted.project.clone.approval.query.result"),
	payload: HostedProjectCloneApprovalQueryResult,
});

export type HostedProjectCloneApprovalQueryResultEnvelope =
	typeof HostedProjectCloneApprovalQueryResultEnvelope.Type;

/** Records an explicit approval or denial for one hosted clone request. */
export const HostedProjectCloneApprovalRespondEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.project.clone.approval.respond"),
	payload: HostedProjectCloneApprovalResponseRequest,
	thread_id: Identifier,
});

export type HostedProjectCloneApprovalRespondEnvelope =
	typeof HostedProjectCloneApprovalRespondEnvelope.Type;

/** Requests preparation of one hosted Git mutation approval. */
export const HostedGitMutationRequestEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.git.mutation.request"),
	payload: HostedGitMutationCommandRequest,
	thread_id: Identifier,
});

export type HostedGitMutationRequestEnvelope = typeof HostedGitMutationRequestEnvelope.Type;

/** Requests one source-free hosted Git mutation approval by durable identity. */
export const HostedGitMutationApprovalQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.git.mutation.approval.query"),
	payload: HostedGitMutationApprovalQuery,
});

export type HostedGitMutationApprovalQueryEnvelope =
	typeof HostedGitMutationApprovalQueryEnvelope.Type;

/** Returns a correlated source-free hosted Git mutation approval projection. */
export const HostedGitMutationApprovalQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("hosted.git.mutation.approval.query.result"),
	payload: HostedGitMutationApprovalQueryResult,
});

export type HostedGitMutationApprovalQueryResultEnvelope =
	typeof HostedGitMutationApprovalQueryResultEnvelope.Type;

/** Records an explicit approval or denial for one hosted Git mutation request. */
export const HostedGitMutationApprovalRespondEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("hosted.git.mutation.approval.respond"),
	payload: HostedGitMutationApprovalResponseRequest,
	thread_id: Identifier,
});

export type HostedGitMutationApprovalRespondEnvelope =
	typeof HostedGitMutationApprovalRespondEnvelope.Type;

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

/** Requests one source-safe tool invocation projection within its owning thread. */
export const ToolInvocationQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("tool.invocation.query"),
	payload: ToolInvocationQuery,
});

export type ToolInvocationQueryEnvelope = typeof ToolInvocationQueryEnvelope.Type;

/** Returns one correlated source-safe tool invocation projection. */
export const ToolInvocationQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("tool.invocation.query.result"),
	payload: ToolInvocationQueryResult,
});

export type ToolInvocationQueryResultEnvelope = typeof ToolInvocationQueryResultEnvelope.Type;

/** Requests one source-safe tool approval projection within its owning thread. */
export const ToolApprovalQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("tool.approval.query"),
	payload: ToolApprovalQuery,
});

export type ToolApprovalQueryEnvelope = typeof ToolApprovalQueryEnvelope.Type;

/** Returns one correlated source-safe tool approval projection. */
export const ToolApprovalQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("tool.approval.query.result"),
	payload: ToolApprovalQueryResult,
});

export type ToolApprovalQueryResultEnvelope = typeof ToolApprovalQueryResultEnvelope.Type;

/** Records one exact-replay decision for a pending tool approval. */
export const ToolApprovalDecideEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("tool.approval.decide"),
	payload: DecideApprovalRequest,
});

export type ToolApprovalDecideEnvelope = typeof ToolApprovalDecideEnvelope.Type;

/** Returns the correlated source-safe approval projection after an exact-replay decision. */
export const ToolApprovalDecideResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("tool.approval.decide.result"),
	payload: DecideApprovalResult,
});

export type ToolApprovalDecideResultEnvelope = typeof ToolApprovalDecideResultEnvelope.Type;

/** Describes the durable work state coordinated for one thread. */
export const ThreadWorkItem = Schema.Struct({
	agent_id: Identifier,
	display_name: Schema.NonEmptyString,
	engine_id: Identifier,
	native_thread_id: Schema.optional(Identifier),
	role: Schema.NonEmptyString,
	run_id: Identifier,
	usage: Schema.optional(RunUsage),
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

/** Returns one provider-neutral orchestration graph projection. */
export const OrchestrationGraphQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("orchestration.graph.query.result"),
	payload: Schema.Struct({ graph: OrchestrationGraph }),
});

export type OrchestrationGraphQueryResultEnvelope =
	typeof OrchestrationGraphQueryResultEnvelope.Type;

/** Requests ordered updates for the thread-list projection. */
export const SubscribeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("subscribe"),
	payload: Schema.Union([
		Schema.Struct({ type: Schema.Literal("thread.list") }),
		Schema.Struct({ type: Schema.Literal("orchestration.graph"), group_id: Identifier }),
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

/** Decodes every client-to-backend frame accepted on the control channel. */
export const InboundControlEnvelope = Schema.Union([
	HelloEnvelope,
	CommandEnvelope,
	ThreadListQueryEnvelope,
	ThreadRetentionQueryEnvelope,
	ThreadRetentionUpdateEnvelope,
	WorkspaceFileReadQueryEnvelope,
	WorkspaceFileReplaceEnvelope,
	WorkspaceChangeReviewEnvelope,
	WorkspaceChangeRollbackEnvelope,
	WorkspaceChangeListQueryEnvelope,
	WorkspaceChangeDiffQueryEnvelope,
	WorkspaceReplaceApprovalQueryEnvelope,
	WorkspaceReplaceApprovalRespondEnvelope,
	WorkspaceGitSessionQueryEnvelope,
	WorkspaceGitSessionRefreshEnvelope,
	WorkspaceGitFetchQueryEnvelope,
	WorkspaceGitFetchPolicyUpdateEnvelope,
	WorkspaceGitFetchRequestEnvelope,
	HostedGitSnapshotQueryEnvelope,
	HostedGitCheckFailureDetailQueryEnvelope,
	HostedGitSnapshotRefreshEnvelope,
	ExternalWaitRequestEnvelope,
	ExternalWaitCancelEnvelope,
	ExternalWaitManualResumeEnvelope,
	ExternalWaitQueryEnvelope,
	PreviewTargetsQueryEnvelope,
	PreviewBrowserLifecycleQueryEnvelope,
	RichLinkMetadataQueryEnvelope,
	WorkspaceGitCheckoutRequestEnvelope,
	WorkspaceGitCheckoutApprovalQueryEnvelope,
	WorkspaceGitCheckoutApprovalRespondEnvelope,
	WorkspaceGitMutationRequestEnvelope,
	WorkspaceGitMutationApprovalQueryEnvelope,
	WorkspaceGitMutationApprovalRespondEnvelope,
	HostedProjectCloneRequestEnvelope,
	HostedProjectCloneApprovalQueryEnvelope,
	HostedProjectCloneApprovalRespondEnvelope,
	HostedGitMutationRequestEnvelope,
	HostedGitMutationApprovalQueryEnvelope,
	HostedGitMutationApprovalRespondEnvelope,
	GlobalGuidanceQueryEnvelope,
	GlobalGuidanceUpdateEnvelope,
	GlobalGuidanceSelectionEnvelope,
	GlobalGuidanceDriftResolutionEnvelope,
	GlobalGuidanceRetryEnvelope,
	ModelBehaviourQueryEnvelope,
	ModelBehaviourUpdateEnvelope,
	ModelBehaviourDriftResolutionEnvelope,
	ModelBehaviourRetryEnvelope,
	ToolInvocationQueryEnvelope,
	ToolApprovalQueryEnvelope,
	ToolApprovalDecideEnvelope,
	ThreadWorkQueryEnvelope,
	TerminalListQueryEnvelope,
	OrchestrationGraphQueryEnvelope,
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
	ThreadListQueryResultEnvelope,
	ThreadRetentionQueryResultEnvelope,
	WorkspaceFileReadQueryResultEnvelope,
	WorkspaceChangeListQueryResultEnvelope,
	WorkspaceChangeDiffQueryResultEnvelope,
	WorkspaceReplaceApprovalQueryResultEnvelope,
	WorkspaceGitSessionQueryResultEnvelope,
	WorkspaceGitFetchQueryResultEnvelope,
	HostedGitSnapshotQueryResultEnvelope,
	HostedGitCheckFailureDetailQueryResultEnvelope,
	ExternalWaitQueryResultEnvelope,
	PreviewTargetsQueryResultEnvelope,
	PreviewBrowserLifecycleQueryResultEnvelope,
	RichLinkMetadataQueryResultEnvelope,
	WorkspaceGitCheckoutApprovalQueryResultEnvelope,
	WorkspaceGitMutationApprovalQueryResultEnvelope,
	HostedProjectCloneApprovalQueryResultEnvelope,
	HostedGitMutationApprovalQueryResultEnvelope,
	GlobalGuidanceQueryResultEnvelope,
	ModelBehaviourQueryResultEnvelope,
	ToolInvocationQueryResultEnvelope,
	ToolApprovalQueryResultEnvelope,
	ToolApprovalDecideResultEnvelope,
	ThreadWorkQueryResultEnvelope,
	TerminalListQueryResultEnvelope,
	OrchestrationGraphQueryResultEnvelope,
	SubscriptionStartedEnvelope,
	SubscriptionStoppedEnvelope,
	ThreadListSnapshotEnvelope,
	ThreadListUpsertEnvelope,
	ThreadListRemoveEnvelope,
	OrchestrationGraphSnapshotEnvelope,
	OrchestrationGraphPatchEnvelope,
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
