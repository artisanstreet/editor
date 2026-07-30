import {
	ArtisanApprovalUpdatedEvent,
	ArtisanAssumptionEvent,
	ArtisanNativeActionEvent,
	ArtisanToolInvocationEvent,
} from "../artisan-tools";

import { ImageAttachmentReference, UserMessageContentPart } from "../attachments";

import {
	Identifier,
	IsoDateTime,
	JournalSequence,
	PositiveInt,
	ProtocolVersion,
	RawOrigin,
	StreamCursor,
	StreamSequence,
} from "../common";

import { GitMutationUpdatedEvent, GitWorkspaceUpdatedEvent } from "../git";

import {
	GlobalGuidanceCanonicalUpdatedEvent,
	GlobalGuidanceProviderReconciledEvent,
	GlobalGuidanceSelectionRequiredEvent,
} from "../guidance";

import { MarketplaceLedgerEvent } from "../marketplace";

import {
	ModelBehaviourProviderReconciledEvent,
	ModelBehaviourSettingUpdatedEvent,
} from "../model-behaviour";

import { ModelFavoritesUpdatedEvent } from "../model-favorites";

import { PreviewInspectionSessionUpdatedEvent, PreviewTargetUpdatedEvent } from "../preview";

import { SessionDefaultsUpdatedEvent } from "../session-defaults";

import {
	ProjectRef,
	ThreadContentErasedEvent,
	ThreadCreatedEvent,
	ThreadErasedEvent,
	ThreadMetadataUpdatedEvent,
	ThreadProjectAffinityIgnoredEvent,
	ThreadProjectAffinityUpdatedEvent,
	ThreadRefinementIgnoredEvent,
	ThreadRetentionPolicyUpdatedEvent,
} from "../thread";

import { WorkspaceChangeUpdatedEvent, WorkspaceConflictUpdatedEvent } from "../workspace-changes";

import {
	AssignmentPermissionPolicy,
	AssignmentScope,
	AssignmentWorkspace,
	CommandPayload,
	GraphActionStatus,
	GraphShortStatus,
	GraphVisibleName,
	GraphVisibleRole,
	ThreadSessionPolicy,
} from "./commands";

import { Schema } from "effect";

import {
	BackendTraceMetadata,
	FrontendTraceMetadata,
	NegotiatedBackendTraceMetadata,
	NegotiatedFrontendTraceMetadata,
} from "./trace";

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
	display_name: GraphVisibleName,
	role: GraphVisibleRole,
	created_at: IsoDateTime,
	updated_at: IsoDateTime,
});

export type AgentInstance = typeof AgentInstance.Type;

/** Describes one compact assignment heartbeat safe for visible projection use. */
export const AssignmentHeartbeat = Schema.Struct({
	short_description: GraphShortStatus,
	current_action: GraphActionStatus,
	blocked_reason: Schema.optional(GraphActionStatus),
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
	role: GraphVisibleRole,
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
	display_name: Schema.optional(GraphVisibleName),
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
	display_name: GraphVisibleName,
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

/** Identifies the public engine/model selected on either side of a continuation. */
export const ThreadModelTransitionEndpoint = Schema.Struct({
	engine_id: Identifier,
	model_id: Schema.optional(Schema.NonEmptyString),
});

export type ThreadModelTransitionEndpoint = typeof ThreadModelTransitionEndpoint.Type;

/** Records a renderer-safe continuation that changes the selected engine or model. */
export const ThreadModelTransitionEvent = Schema.Struct({
	continuation: Schema.Literals(["native", "portable"]),
	source: ThreadModelTransitionEndpoint,
	target: ThreadModelTransitionEndpoint,
	type: Schema.Literal("thread.model_transition"),
});

export type ThreadModelTransitionEvent = typeof ThreadModelTransitionEvent.Type;

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
	ModelFavoritesUpdatedEvent,
	SessionDefaultsUpdatedEvent,
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
	ThreadModelTransitionEvent,
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
