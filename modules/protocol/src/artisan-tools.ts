import { Schema } from "effect";

import { Identifier, IsoDateTime, JournalSequence, PositiveInt, RawOrigin } from "./common";
import {
	GitDiffQuery,
	GitIndexStageRequest,
	GitIndexUnstageRequest,
	GitWorkspaceQuery,
} from "./git";
import {
	WorkspaceFileReadQuery,
	WorkspaceFileReplaceRequest,
	WorkspacePath,
} from "./workspace-changes";

const text_encoder = new TextEncoder();

const EnvironmentVariableName = Schema.String.check(
	Schema.makeFilter<string>((name) =>
		name.length === 0 || name.includes("=") || name.includes(String.fromCharCode(0))
			? "Expected a non-empty environment variable name without equals or null"
			: undefined,
	),
);

const BoundedText = (maximum_bytes: number, label: string) =>
	Schema.String.check(
		Schema.makeFilter<string>((value) =>
			text_encoder.encode(value).byteLength <= maximum_bytes
				? undefined
				: `Expected at most ${maximum_bytes} UTF-8 bytes for ${label}`,
		),
	);

/** Identifies one built-in control-plane tool without exposing provider tool names. */
export const ArtisanToolId = Schema.Literals([
	"question.ask",
	"assumption.record",
	"terminal.open",
	"terminal.read",
	"terminal.write",
	"terminal.restart",
	"terminal.stop",
	"workspace.file.read",
	"workspace.file.list",
	"workspace.file.write",
	"workspace.language.status",
	"git.status.read",
	"git.diff.read",
	"git.index.stage",
	"git.index.unstage",
	"preview.open",
	"preview.inspect",
	"preview.stop",
	"approval.request",
	"engine.native_action.record",
]);

export type ArtisanToolId = typeof ArtisanToolId.Type;

/** Groups a built-in tool for canonical Artisan navigation and policy. */
export const ArtisanToolKind = Schema.Literals([
	"question",
	"assumption",
	"terminal",
	"workspace_file",
	"git",
	"preview",
	"approval",
	"native_action",
]);

export type ArtisanToolKind = typeof ArtisanToolKind.Type;

/** Declares the action category a policy may allow, require approval for, or deny. */
export const PermissionRequirement = Schema.Literals([
	"none",
	"workspace_read",
	"workspace_write",
	"process_control",
	"git_read",
	"git_index_write",
	"preview_control",
	"user_interaction",
	"engine_observation",
]);

export type PermissionRequirement = typeof PermissionRequirement.Type;

/** Describes one stable built-in tool offered to an engine-facing registry. */
const ArtisanToolDescriptorBase = Schema.Struct({
	approval_behavior: Schema.Literals(["never", "on_request"]),
	description: BoundedText(4_096, "tool description"),
	id: ArtisanToolId,
	kind: ArtisanToolKind,
	permission_requirements: Schema.Array(PermissionRequirement).check(Schema.isMaxLength(8)),
	schema_version: Schema.Literal(1),
	title: BoundedText(256, "tool title").check(Schema.isMinLength(1)),
});

const tool_kind_for_id: Record<ArtisanToolId, ArtisanToolKind> = {
	"approval.request": "approval",
	"assumption.record": "assumption",
	"engine.native_action.record": "native_action",
	"git.diff.read": "git",
	"git.index.stage": "git",
	"git.index.unstage": "git",
	"git.status.read": "git",
	"preview.inspect": "preview",
	"preview.open": "preview",
	"preview.stop": "preview",
	"question.ask": "question",
	"terminal.open": "terminal",
	"terminal.read": "terminal",
	"terminal.restart": "terminal",
	"terminal.stop": "terminal",
	"terminal.write": "terminal",
	"workspace.file.read": "workspace_file",
	"workspace.file.list": "workspace_file",
	"workspace.file.write": "workspace_file",
	"workspace.language.status": "workspace_file",
};

export const ArtisanToolDescriptor = ArtisanToolDescriptorBase.check(
	Schema.makeFilter<typeof ArtisanToolDescriptorBase.Type>((descriptor) =>
		tool_kind_for_id[descriptor.id] === descriptor.kind
			? undefined
			: "Expected the canonical kind for the declared built-in tool id",
	),
);

export type ArtisanToolDescriptor = typeof ArtisanToolDescriptor.Type;

/** Captures the session policy selected before a tool invocation is routed. */
export const ArtisanToolPermissionPolicy = Schema.Struct({
	approval: Schema.Literals(["never", "on_request", "always"]),
	allow_engine_observation: Schema.Boolean,
	allow_git_index_write: Schema.Boolean,
	allow_preview_control: Schema.Boolean,
	allow_process_control: Schema.Boolean,
	allow_workspace_read: Schema.Boolean,
	allow_workspace_write: Schema.Boolean,
});

export type ArtisanToolPermissionPolicy = typeof ArtisanToolPermissionPolicy.Type;

/** Records the policy result for one requested built-in tool action. */
export const ArtisanToolPermissionDecision = Schema.Struct({
	decision: Schema.Literals(["allowed", "approval_required", "denied"]),
	policy: ArtisanToolPermissionPolicy,
	reason: Schema.optional(BoundedText(2_048, "permission decision reason")),
	requirements: Schema.Array(PermissionRequirement).check(Schema.isMaxLength(8)),
	tool_id: ArtisanToolId,
});

export type ArtisanToolPermissionDecision = typeof ArtisanToolPermissionDecision.Type;

/** Defines the durable lifecycle of a tool invocation. */
export const ArtisanToolInvocationLifecycle = Schema.Literals([
	"requested",
	"awaiting_approval",
	"running",
	"succeeded",
	"denied",
	"failed",
	"cancelled",
	"unsupported",
]);

export type ArtisanToolInvocationLifecycle = typeof ArtisanToolInvocationLifecycle.Type;

/** Provides a bounded, safe-to-display outcome for an invocation. */
export const ArtisanToolInvocationOutcome = Schema.Struct({
	code: Identifier,
	detail: Schema.optional(BoundedText(8 * 1024, "tool outcome detail")),
	status: Schema.Literals(["succeeded", "denied", "failed", "cancelled", "unsupported"]),
});

export type ArtisanToolInvocationOutcome = typeof ArtisanToolInvocationOutcome.Type;

/** Binds a controlled invocation to durable workspace evidence without leaking raw payloads. */
export const WorkspaceEvidenceBinding = Schema.Struct({
	operation_id: Identifier,
	recorder: Schema.Literal("workspace_evidence"),
	recorded_event_type: Schema.Literals([
		"filesystem.mutation",
		"git.workspace.observed",
		"process.ownership",
	]),
});

export type WorkspaceEvidenceBinding = typeof WorkspaceEvidenceBinding.Type;

/** Projects one observable, attributable built-in or normalized engine action. */
const ArtisanToolInvocationBase = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	approval_id: Schema.optional(Identifier),
	completed_at: Schema.optional(IsoDateTime),
	input_summary: BoundedText(16 * 1024, "tool invocation input summary").check(
		Schema.isMinLength(1),
	),
	invocation_id: Identifier,
	lifecycle: ArtisanToolInvocationLifecycle,
	outcome: Schema.optional(ArtisanToolInvocationOutcome),
	permission: ArtisanToolPermissionDecision,
	raw_origin: Schema.optional(RawOrigin),
	requested_at: IsoDateTime,
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
	tool_id: ArtisanToolId,
	updated_at: IsoDateTime,
	workspace_evidence: Schema.optional(WorkspaceEvidenceBinding),
});

/** Projects one observable, attributable built-in or normalized engine action. */
export const ArtisanToolInvocation = ArtisanToolInvocationBase.check(
	Schema.makeFilter<typeof ArtisanToolInvocationBase.Type>((invocation) => {
		const terminal_lifecycle = new Set([
			"succeeded",
			"denied",
			"failed",
			"cancelled",
			"unsupported",
		]);

		return invocation.permission.tool_id !== invocation.tool_id
			? "Expected the permission decision to bind the invoked tool"
			: invocation.outcome !== undefined && invocation.outcome.status !== invocation.lifecycle
				? "Expected a terminal invocation lifecycle to match its outcome status"
				: invocation.outcome !== undefined && !terminal_lifecycle.has(invocation.lifecycle)
					? "Expected an outcome only after a terminal invocation lifecycle"
					: invocation.lifecycle === "awaiting_approval" &&
						  invocation.approval_id === undefined
						? "Expected awaiting approval invocations to bind an approval"
						: undefined;
	}),
);

export type ArtisanToolInvocation = typeof ArtisanToolInvocation.Type;

/** Announces an append-only lifecycle transition for one tool invocation. */
export const ArtisanToolInvocationEvent = Schema.Struct({
	invocation: ArtisanToolInvocation,
	type: Schema.Literal("artisan.tool.invocation.updated"),
});

export type ArtisanToolInvocationEvent = typeof ArtisanToolInvocationEvent.Type;

/** Describes one bounded answer field for a canonical user question. */
export const ArtisanQuestionField = Schema.Struct({
	field_id: Identifier,
	label: BoundedText(1_024, "question field label").check(Schema.isMinLength(1)),
	multiple: Schema.Boolean,
	required: Schema.Boolean,
});

export type ArtisanQuestionField = typeof ArtisanQuestionField.Type;

/** Carries a durable, engine-neutral clarification question before risky work proceeds. */
export const ArtisanQuestionRequest = Schema.Struct({
	fields: Schema.NonEmptyArray(ArtisanQuestionField).check(Schema.isMaxLength(16)),
	invocation_id: Identifier,
	question_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	text: BoundedText(16 * 1024, "question text").check(Schema.isMinLength(1)),
});

export type ArtisanQuestionRequest = typeof ArtisanQuestionRequest.Type;

/** Records a safe low-risk assumption made by the harness or an engine. */
export const ArtisanAssumptionEvent = Schema.Struct({
	assumption_id: Identifier,
	invocation_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	reason: Schema.optional(BoundedText(4_096, "assumption reason")),
	statement: BoundedText(16 * 1024, "assumption statement").check(Schema.isMinLength(1)),
	type: Schema.Literal("artisan.assumption.recorded"),
});

export type ArtisanAssumptionEvent = typeof ArtisanAssumptionEvent.Type;

/** Requests explicit approval before a policy-sensitive action is dispatched. */
export const ArtisanApprovalRequest = Schema.Struct({
	approval_id: Identifier,
	description: BoundedText(16 * 1024, "approval description").check(Schema.isMinLength(1)),
	invocation_id: Identifier,
	permission_requirements: Schema.Array(PermissionRequirement).check(Schema.isMinLength(1)),
	raw_origin: Schema.optional(RawOrigin),
	requested_at: IsoDateTime,
});

export type ArtisanApprovalRequest = typeof ArtisanApprovalRequest.Type;

/** Records the final user decision for an exactly bound approval request. */
export const ArtisanApprovalResolution = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	invocation_id: Identifier,
	resolved_at: IsoDateTime,
	resolution_id: Identifier,
});

export type ArtisanApprovalResolution = typeof ArtisanApprovalResolution.Type;

/** Projects a durable approval interaction for reload-safe user-visible pending work. */
const ArtisanApprovalProjectionBase = Schema.Struct({
	request: ArtisanApprovalRequest,
	resolution: Schema.optional(ArtisanApprovalResolution),
	state: Schema.Literals(["pending", "resolved"]),
});

export const ArtisanApprovalProjection = ArtisanApprovalProjectionBase.check(
	Schema.makeFilter<typeof ArtisanApprovalProjectionBase.Type>((approval) =>
		(approval.state === "pending" && approval.resolution === undefined) ||
		(approval.state === "resolved" && approval.resolution !== undefined)
			? undefined
			: "Expected approval state to agree with the durable resolution",
	),
);

export type ArtisanApprovalProjection = typeof ArtisanApprovalProjection.Type;

/** Announces creation or final resolution of a durable approval interaction. */
export const ArtisanApprovalUpdatedEvent = Schema.Struct({
	approval: ArtisanApprovalProjection,
	type: Schema.Literal("artisan.approval.updated"),
});

export type ArtisanApprovalUpdatedEvent = typeof ArtisanApprovalUpdatedEvent.Type;

/** Normalizes an observable engine-owned action that has no direct Artisan implementation. */
export const ArtisanNativeActionEvent = Schema.Struct({
	action: BoundedText(512, "native action name").check(Schema.isMinLength(1)),
	detail: Schema.optional(BoundedText(8 * 1024, "native action detail")),
	invocation_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	tool_id: Schema.Literal("engine.native_action.record"),
	type: Schema.Literal("engine.native_action"),
});

export type ArtisanNativeActionEvent = typeof ArtisanNativeActionEvent.Type;

/** Declares the supported input/output surface for one registry tool without provider payloads. */
export const ArtisanToolDeclaration = Schema.Struct({
	descriptor: ArtisanToolDescriptor,
	input_schema_version: PositiveInt,
	output_schema_version: PositiveInt,
});

export type ArtisanToolDeclaration = typeof ArtisanToolDeclaration.Type;

/** Requests the built-in declarations available to one policy-aware backend registry. */
export const ArtisanToolRegistryListQuery = Schema.Struct({
	policy: ArtisanToolPermissionPolicy,
	thread_id: Schema.optional(Identifier),
	workspace_id: Schema.optional(Identifier),
});

export type ArtisanToolRegistryListQuery = typeof ArtisanToolRegistryListQuery.Type;

/** Describes the controlled workspace-file target carried by a registry execution request. */
export const ArtisanToolWorkspaceFileTarget = Schema.Struct({
	path: WorkspacePath,
	workspace_id: Identifier,
});

export type ArtisanToolWorkspaceFileTarget = typeof ArtisanToolWorkspaceFileTarget.Type;

/** Requests a bounded, content-free recursive workspace path projection for editor discovery. */
export const WorkspaceFileDiscoveryQuery = Schema.Struct({
	after_path: Schema.optional(WorkspacePath),
	/**
	 * How many levels below the prefix to walk. A file tree opens one level at a
	 * time, and an unbounded walk of a real repository spends its whole page on
	 * whichever subtree sorts first. Absent means the walk is bounded only by
	 * `limit`.
	 */
	depth: Schema.optional(
		PositiveInt.check(
			Schema.isLessThanOrEqualTo(64, { message: "Expected at most 64 levels" }),
		),
	),
	limit: Schema.optional(
		PositiveInt.check(
			Schema.isLessThanOrEqualTo(1_000, { message: "Expected at most 1,000 paths" }),
		),
	),
	prefix: Schema.optional(WorkspacePath),
	workspace_id: Identifier,
});

export type WorkspaceFileDiscoveryQuery = typeof WorkspaceFileDiscoveryQuery.Type;

/** Describes bounded path metadata without exposing filesystem content or absolute paths. */
export const WorkspaceFileDiscoveryEntry = Schema.Struct({
	kind: Schema.Literals(["directory", "file"]),
	modified_at: IsoDateTime,
	path: WorkspacePath,
	size: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
});

export type WorkspaceFileDiscoveryEntry = typeof WorkspaceFileDiscoveryEntry.Type;

/** Returns one deterministic discovery page from a root-confined backend filesystem. */
export const WorkspaceFileDiscoveryQueryResult = Schema.Struct({
	entries: Schema.Array(WorkspaceFileDiscoveryEntry).check(Schema.isMaxLength(1_000)),
	next_path: Schema.optional(WorkspacePath),
	truncated: Schema.Boolean,
	workspace_id: Identifier,
});

export type WorkspaceFileDiscoveryQueryResult = typeof WorkspaceFileDiscoveryQueryResult.Type;

/** Requests the truthful editor-language and diagnostics support state for one workspace. */
export const WorkspaceLanguageCapabilitiesQuery = Schema.Struct({
	workspace_id: Identifier,
});

export type WorkspaceLanguageCapabilitiesQuery = typeof WorkspaceLanguageCapabilitiesQuery.Type;

/** Projects one backend-owned language feature without implying unavailable infrastructure exists. */
export const WorkspaceLanguageCapability = Schema.Struct({
	feature: Schema.Literals(["diagnostics", "language_detection", "symbols"]),
	reason: Schema.optional(BoundedText(2_048, "language capability reason")),
	source: Schema.Literals(["artisan", "engine", "language_server", "unavailable"]),
	state: Schema.Literals(["available", "unavailable"]),
});

export type WorkspaceLanguageCapability = typeof WorkspaceLanguageCapability.Type;

/** Returns truthful language support states consumed by Monaco without direct host access. */
export const WorkspaceLanguageCapabilitiesQueryResult = Schema.Struct({
	capabilities: Schema.Array(WorkspaceLanguageCapability).check(Schema.isMaxLength(16)),
	workspace_id: Identifier,
});

export type WorkspaceLanguageCapabilitiesQueryResult =
	typeof WorkspaceLanguageCapabilitiesQueryResult.Type;

/** Declares whether a built-in tool is available under the current workspace and policy. */
export const ArtisanToolAvailability = Schema.Struct({
	state: Schema.Literals(["available", "approval_required", "unavailable"]),
	tool_id: ArtisanToolId,
	unavailable_reason: Schema.optional(BoundedText(2_048, "tool availability reason")),
});

export type ArtisanToolAvailability = typeof ArtisanToolAvailability.Type;

/** Provides compact renderer-safe usage data without retaining provider tool payloads. */
export const ArtisanToolUsage = Schema.Struct({
	active_invocation_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(1_000_000),
	),
	last_invoked_at: Schema.optional(IsoDateTime),
	tool_id: ArtisanToolId,
	total_invocation_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(1_000_000_000),
	),
});

export type ArtisanToolUsage = typeof ArtisanToolUsage.Type;

/** Returns the bounded built-in registry snapshot at a durable ledger position. */
export const ArtisanToolRegistryListQueryResult = Schema.Struct({
	availability: Schema.Array(ArtisanToolAvailability).check(Schema.isMaxLength(32)),
	declarations: Schema.Array(ArtisanToolDeclaration).check(Schema.isMaxLength(32)),
	journal_sequence: JournalSequence,
	usage: Schema.Array(ArtisanToolUsage).check(Schema.isMaxLength(32)),
});

export type ArtisanToolRegistryListQueryResult = typeof ArtisanToolRegistryListQueryResult.Type;

const TerminalOpenInput = Schema.Struct({
	args: Schema.Array(BoundedText(8 * 1024, "terminal argument")).check(Schema.isMaxLength(128)),
	cols: PositiveInt,
	env: Schema.optional(Schema.Record(EnvironmentVariableName, Schema.String)),
	executable: BoundedText(16 * 1024, "terminal executable").check(Schema.isMinLength(1)),
	rows: PositiveInt,
	terminal_id: Identifier,
	tool_id: Schema.Literal("terminal.open"),
	working_directory: BoundedText(16 * 1024, "terminal working directory").check(
		Schema.isMinLength(1),
	),
	workspace_id: Identifier,
});

const TerminalReadInput = Schema.Struct({
	terminal_id: Identifier,
	tool_id: Schema.Literal("terminal.read"),
});

const TerminalWriteInput = Schema.Struct({
	data: BoundedText(64 * 1024, "terminal input"),
	terminal_id: Identifier,
	tool_id: Schema.Literal("terminal.write"),
});

const TerminalRestartInput = Schema.Struct({
	terminal_id: Identifier,
	tool_id: Schema.Literal("terminal.restart"),
});

const TerminalStopInput = Schema.Struct({
	signal: Schema.optional(BoundedText(128, "terminal stop signal")),
	terminal_id: Identifier,
	tool_id: Schema.Literal("terminal.stop"),
});

/** Carries the minimal action-specific request parameters for one built-in tool. */
export const ArtisanToolExecutionInput = Schema.Union([
	Schema.Struct({
		fields: ArtisanQuestionRequest.fields.fields,
		question_id: ArtisanQuestionRequest.fields.question_id,
		text: ArtisanQuestionRequest.fields.text,
		tool_id: Schema.Literal("question.ask"),
	}),
	Schema.Struct({
		assumption_id: ArtisanAssumptionEvent.fields.assumption_id,
		reason: ArtisanAssumptionEvent.fields.reason,
		statement: ArtisanAssumptionEvent.fields.statement,
		tool_id: Schema.Literal("assumption.record"),
	}),
	TerminalOpenInput,
	TerminalReadInput,
	TerminalWriteInput,
	TerminalRestartInput,
	TerminalStopInput,
	Schema.Struct({
		...WorkspaceFileReadQuery.fields,
		tool_id: Schema.Literal("workspace.file.read"),
	}),
	Schema.Struct({
		...WorkspaceFileDiscoveryQuery.fields,
		tool_id: Schema.Literal("workspace.file.list"),
	}),
	Schema.Struct({
		...WorkspaceFileReplaceRequest.fields,
		tool_id: Schema.Literal("workspace.file.write"),
	}),
	Schema.Struct({
		...WorkspaceLanguageCapabilitiesQuery.fields,
		tool_id: Schema.Literal("workspace.language.status"),
	}),
	Schema.Struct({ ...GitWorkspaceQuery.fields, tool_id: Schema.Literal("git.status.read") }),
	Schema.Struct({ ...GitDiffQuery.fields, tool_id: Schema.Literal("git.diff.read") }),
	Schema.Struct({ ...GitIndexStageRequest.fields, tool_id: Schema.Literal("git.index.stage") }),
	Schema.Struct({
		...GitIndexUnstageRequest.fields,
		tool_id: Schema.Literal("git.index.unstage"),
	}),
	Schema.Struct({
		preview_id: Identifier,
		tool_id: Schema.Literal("preview.open"),
		url: BoundedText(16 * 1024, "preview URL").check(Schema.isMinLength(1)),
	}),
	Schema.Struct({ preview_id: Identifier, tool_id: Schema.Literal("preview.inspect") }),
	Schema.Struct({ preview_id: Identifier, tool_id: Schema.Literal("preview.stop") }),
	Schema.Struct({
		approval_id: ArtisanApprovalRequest.fields.approval_id,
		description: ArtisanApprovalRequest.fields.description,
		permission_requirements: ArtisanApprovalRequest.fields.permission_requirements,
		tool_id: Schema.Literal("approval.request"),
	}),
	Schema.Struct({
		action: ArtisanNativeActionEvent.fields.action,
		detail: ArtisanNativeActionEvent.fields.detail,
		tool_id: Schema.Literal("engine.native_action.record"),
	}),
]);

export type ArtisanToolExecutionInput = typeof ArtisanToolExecutionInput.Type;

/** Starts one policy-routed built-in execution with exact action-specific command data. */
export const ArtisanToolExecutionRequest = Schema.Struct({
	input: ArtisanToolExecutionInput,
	invocation_id: Identifier,
	policy: ArtisanToolPermissionPolicy,
	raw_origin: Schema.optional(RawOrigin),
});

export type ArtisanToolExecutionRequest = typeof ArtisanToolExecutionRequest.Type;

/** Resolves an exact pending approval from a renderer-safe command surface. */
export const ArtisanApprovalResolveRequest = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	invocation_id: Identifier,
	resolution_id: Identifier,
});

export type ArtisanApprovalResolveRequest = typeof ArtisanApprovalResolveRequest.Type;

/** Requests an attributed bounded invocation history for one thread. */
export const ArtisanToolInvocationListQuery = Schema.Struct({
	after_journal_sequence: Schema.optional(JournalSequence),
	lifecycle: Schema.optional(ArtisanToolInvocationLifecycle),
	limit: Schema.optional(
		PositiveInt.check(
			Schema.isLessThanOrEqualTo(100, { message: "Expected at most 100 items" }),
		),
	),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
	tool_id: Schema.optional(ArtisanToolId),
});

export type ArtisanToolInvocationListQuery = typeof ArtisanToolInvocationListQuery.Type;

/** Returns invocations and a durable cursor suitable for a renderer subscription catch-up. */
export const ArtisanToolInvocationListQueryResult = Schema.Struct({
	invocations: Schema.Array(ArtisanToolInvocation).check(Schema.isMaxLength(100)),
	journal_sequence: JournalSequence,
});

export type ArtisanToolInvocationListQueryResult = typeof ArtisanToolInvocationListQueryResult.Type;

/** Requests pending and resolved approvals belonging to one current thread. */
export const ArtisanApprovalListQuery = Schema.Struct({
	state: Schema.optional(Schema.Literals(["pending", "resolved"])),
	thread_id: Identifier,
});

export type ArtisanApprovalListQuery = typeof ArtisanApprovalListQuery.Type;

/** Returns durable approval interactions and the consistent ledger position they reflect. */
export const ArtisanApprovalListQueryResult = Schema.Struct({
	approvals: Schema.Array(ArtisanApprovalProjection).check(Schema.isMaxLength(100)),
	journal_sequence: JournalSequence,
});

export type ArtisanApprovalListQueryResult = typeof ArtisanApprovalListQueryResult.Type;
