import {
	ImageAttachmentUpload,
	MaximumImageAttachmentCount,
	UserMessageInputContentPart,
} from "../attachments";

import { Identifier, IsoDateTime, PositiveInt } from "../common";

import { ModelFavoriteUpdateCommand } from "../model-favorites";

import { SessionDefaultsUpdateCommand } from "../session-defaults";

import {
	ThreadActivityRecordCommand,
	ThreadAttentionAcknowledgeCommand,
	ThreadArchiveCommand,
	ThreadCreateCommand,
	ThreadMetadataRefineCommand,
	ThreadPinCommand,
	ThreadProjectAssignCommand,
	ThreadProjectUnlockCommand,
	ThreadRenameCommand,
	ThreadRestoreCommand,
	ThreadRetentionUpdateCommand,
	ThreadUnpinCommand,
} from "../thread";

import { Effect, Schema } from "effect";

const EnvironmentVariableName = Schema.String.check(
	Schema.makeFilter<string>((name) =>
		name.length === 0 || name.includes("=") || name.includes(String.fromCharCode(0))
			? "Expected a non-empty environment variable name without equals or null"
			: undefined,
	),
);

/** Queues user text for a thread or steers its active capable run. */
export const ThreadSendMessageCommand = Schema.Struct({
	attachments: Schema.optional(
		Schema.Array(ImageAttachmentUpload).check(Schema.isMaxLength(MaximumImageAttachmentCount)),
	),
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
	/**
	 * The native context-window suffix appended to the model id (for example
	 * Claude Code's `[1m]`). Absent means the harness resolves the bare model
	 * id itself, which lands on the catalog capability's default option — for
	 * Claude 5 that is the extended window, not the 200K base one.
	 */
	context_window: Schema.optional(Schema.NonEmptyString),
	reasoning_effort: Schema.Literals(["low", "medium", "high", "xhigh", "max", "ultra"]),
	/**
	 * The harness-neutral permission option id from the catalog manifest, and
	 * the authoritative permission choice. `permission_mode` and `sandbox_mode`
	 * are the two coarse axes derived from it for sandbox and tool gating; they
	 * cannot express every harness option (Claude alone has five), so nothing
	 * may reconstruct the option id from them.
	 */
	permission: Identifier.pipe(
		Schema.optional,
		Schema.withDecodingDefault(Effect.succeed("supervised")),
	),
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

/**
 * Reads the neutral permission option id a policy stands for. Policies written
 * before the field existed carry only the two coarse axes, so they resolve to
 * the neutral option those axes described.
 */
export const SessionPolicyPermission = (policy: ThreadSessionPolicy) =>
	policy.permission ??
	(policy.sandbox_mode === "read_only"
		? "restricted"
		: policy.permission_mode === "never"
			? "autonomous"
			: "supervised");

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
		const code = character.codePointAt(0) ?? 0;

		return code <= 31 || code === 127 || (code >= 128 && code <= 159);
	});

export const GraphVisibleName = Schema.String.check(
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

export const GraphVisibleRole = Schema.String.check(
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

const GraphStatusInput = Schema.String.check(
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

export const GraphShortStatus = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 && value.length <= 160 && !has_graph_control_character(value)
			? undefined
			: "Expected at most 160 visible status characters",
	),
);

export const GraphActionStatus = Schema.String.check(
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
	display_name: Schema.optional(GraphVisibleName),
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
	name_bank: Schema.optional(Schema.NonEmptyArray(GraphVisibleName)),
	max_concurrency: Schema.optional(BoundedOrchestrationConcurrency),
});

/** Renames an Artisan-owned agent identity inside one visible group. */
export const AgentInstanceRenameCommand = Schema.Struct({
	type: Schema.Literal("agent_instance.rename"),
	group_id: Identifier,
	agent_id: Identifier,
	display_name: GraphVisibleName,
});

/** Records a compact, observable status heartbeat without private reasoning. */
export const AssignmentHeartbeatCommand = Schema.Struct({
	type: Schema.Literal("assignment.heartbeat"),
	group_id: Identifier,
	assignment_id: Identifier,
	short_description: GraphStatusInput,
	current_action: GraphStatusInput,
	blocked_reason: Schema.optional(GraphStatusInput),
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
	ThreadAttentionAcknowledgeCommand,
	ThreadActivityRecordCommand,
	ThreadPinCommand,
	ThreadUnpinCommand,
	ThreadArchiveCommand,
	ThreadRestoreCommand,
	ThreadRetentionUpdateCommand,
	ModelFavoriteUpdateCommand,
	SessionDefaultsUpdateCommand,
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
