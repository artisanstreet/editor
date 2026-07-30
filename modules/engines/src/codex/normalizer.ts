import { Effect, Schema } from "effect";

import { TokenCount } from "../engine";

import type {
	EngineAgentMessageCompletedObservation,
	EngineAgentMessageDeltaObservation,
	EngineAssistantMessagePhase,
	EngineApprovalObservation,
	EngineCompactionObservation,
	EngineFileObservation,
	EngineNativeActionObservation,
	EngineObservation,
	EnginePlanObservation,
	EngineProtocolDiagnosticObservation,
	EngineQuestionObservation,
	EngineRawProvenance,
	EngineReasoningSummaryDeltaObservation,
	EngineRetryObservation,
	EngineRunStateObservation,
	EngineSearchObservation,
	EngineTerminalActivityObservation,
	EngineToolObservation,
	EngineTurnStateObservation,
	EngineUsageObservation,
} from "../engine";

/** Supplies immutable context used to annotate one decoded Codex frame. @since 0.3.0 */
export interface CodexNormalizationInput {
	readonly artisan_run_id: string;
	readonly frame_sequence: number;
	readonly id?: string | number;
	readonly method: string;
	readonly payload: unknown;
	readonly protocol_version: string;
	readonly raw_frame_base64: string;
	readonly transport: string;
}

const ThreadStatusSchema = Schema.Union([
	Schema.Struct({ type: Schema.Literal("notLoaded") }),
	Schema.Struct({ type: Schema.Literal("idle") }),
	Schema.Struct({ type: Schema.Literal("systemError") }),
	Schema.Struct({
		activeFlags: Schema.Array(Schema.Literals(["waitingOnApproval", "waitingOnUserInput"])),
		type: Schema.Literal("active"),
	}),
]);

const TurnSchema = Schema.Struct({
	id: Schema.String,
	status: Schema.Literals(["completed", "failed", "inProgress", "interrupted"]),
});

const ThreadStartedSchema = Schema.Struct({
	thread: Schema.Struct({ id: Schema.String, status: ThreadStatusSchema }),
});
const ThreadStatusSchemaEnvelope = Schema.Struct({
	status: ThreadStatusSchema,
	threadId: Schema.String,
});
const TurnEnvelopeSchema = Schema.Struct({ threadId: Schema.String, turn: TurnSchema });
const AgentDeltaSchema = Schema.Struct({
	delta: Schema.String,
	itemId: Schema.String,
	threadId: Schema.String,
	turnId: Schema.String,
});
const PlanSchema = Schema.Struct({
	explanation: Schema.NullOr(Schema.String),
	plan: Schema.Array(
		Schema.Struct({
			status: Schema.Literals(["completed", "inProgress", "pending"]),
			step: Schema.String,
		}),
	),
	threadId: Schema.String,
	turnId: Schema.String,
});
const AgentMessageItemSchema = Schema.Struct({
	id: Schema.String,
	memoryCitation: Schema.NullOr(Schema.Unknown),
	phase: Schema.NullOr(Schema.String),
	text: Schema.String,
	type: Schema.Literal("agentMessage"),
});

const assistant_message_phase = (phase: string | null): EngineAssistantMessagePhase =>
	phase === "commentary" || phase === "final" ? phase : "unspecified";
const PlanItemSchema = Schema.Struct({
	id: Schema.String,
	text: Schema.String,
	type: Schema.Literal("plan"),
});
const CommandItemSchema = Schema.Struct({
	aggregatedOutput: Schema.NullOr(Schema.String),
	command: Schema.String,
	commandActions: Schema.Array(Schema.Unknown),
	cwd: Schema.String,
	durationMs: Schema.NullOr(Schema.Number),
	exitCode: Schema.NullOr(Schema.Number),
	id: Schema.String,
	processId: Schema.NullOr(Schema.String),
	source: Schema.Unknown,
	status: Schema.Literals(["completed", "declined", "failed", "inProgress"]),
	type: Schema.Literal("commandExecution"),
});
const FileItemSchema = Schema.Struct({
	changes: Schema.Array(
		Schema.Struct({
			diff: Schema.String,
			kind: Schema.Union([
				Schema.Struct({ type: Schema.Literal("add") }),
				Schema.Struct({ type: Schema.Literal("delete") }),
				Schema.Struct({
					move_path: Schema.NullOr(Schema.String),
					type: Schema.Literal("update"),
				}),
			]),
			path: Schema.String,
		}),
	),
	id: Schema.String,
	status: Schema.Literals(["completed", "declined", "failed", "inProgress"]),
	type: Schema.Literal("fileChange"),
});
const McpItemSchema = Schema.Struct({
	appContext: Schema.NullOr(Schema.Unknown),
	arguments: Schema.Unknown,
	durationMs: Schema.NullOr(Schema.Number),
	error: Schema.NullOr(Schema.Unknown),
	id: Schema.String,
	pluginId: Schema.NullOr(Schema.String),
	result: Schema.NullOr(Schema.Unknown),
	server: Schema.String,
	status: Schema.Literals(["completed", "failed", "inProgress"]),
	tool: Schema.String,
	type: Schema.Literal("mcpToolCall"),
});
const DynamicToolItemSchema = Schema.Struct({
	arguments: Schema.Unknown,
	contentItems: Schema.NullOr(Schema.Array(Schema.Unknown)),
	durationMs: Schema.NullOr(Schema.Number),
	id: Schema.String,
	namespace: Schema.NullOr(Schema.String),
	status: Schema.Literals(["completed", "failed", "inProgress"]),
	success: Schema.NullOr(Schema.Boolean),
	tool: Schema.String,
	type: Schema.Literal("dynamicToolCall"),
});
const WebSearchItemSchema = Schema.Struct({
	action: Schema.NullOr(Schema.Unknown),
	id: Schema.String,
	query: Schema.String,
	type: Schema.Literal("webSearch"),
});
const SubagentItemSchema = Schema.Struct({
	agentPath: Schema.String,
	agentThreadId: Schema.String,
	id: Schema.String,
	kind: Schema.Unknown,
	type: Schema.Literal("subAgentActivity"),
});
const ContextCompactionItemSchema = Schema.Struct({
	id: Schema.String,
	type: Schema.Literal("contextCompaction"),
});
const ItemEnvelopeSchema = Schema.Struct({
	item: Schema.Union([
		AgentMessageItemSchema,
		PlanItemSchema,
		CommandItemSchema,
		FileItemSchema,
		McpItemSchema,
		DynamicToolItemSchema,
		WebSearchItemSchema,
		SubagentItemSchema,
		ContextCompactionItemSchema,
	]),
	threadId: Schema.String,
	turnId: Schema.String,
});
const CommandOutputSchema = Schema.Struct({
	delta: Schema.String,
	itemId: Schema.String,
	threadId: Schema.String,
	turnId: Schema.String,
});
const FileOutputSchema = CommandOutputSchema;
const McpProgressSchema = Schema.Struct({
	itemId: Schema.String,
	message: Schema.String,
	threadId: Schema.String,
	turnId: Schema.String,
});
const ReasoningSummaryDeltaSchema = Schema.Struct({
	delta: Schema.String,
	itemId: Schema.String,
	summaryIndex: Schema.Int,
	threadId: Schema.String,
	turnId: Schema.String,
});
const PrivateReasoningDeltaSchema = Schema.Struct({
	contentIndex: Schema.Int,
	delta: Schema.String,
	itemId: Schema.String,
	threadId: Schema.String,
	turnId: Schema.String,
});
const UsageSchema = Schema.Struct({
	threadId: Schema.String,
	tokenUsage: Schema.Struct({
		total: Schema.Struct({
			cachedInputTokens: Schema.optional(TokenCount),
			inputTokens: Schema.optional(TokenCount),
			outputTokens: Schema.optional(TokenCount),
		}),
	}),
	turnId: Schema.String,
});
const CompactionSchema = Schema.Struct({ threadId: Schema.String, turnId: Schema.String });
const ErrorSchema = Schema.Struct({
	error: Schema.Struct({ message: Schema.String }),
	threadId: Schema.String,
	turnId: Schema.String,
	willRetry: Schema.Boolean,
});
const WarningSchema = Schema.Struct({
	message: Schema.String,
	threadId: Schema.NullOr(Schema.String),
});
const ModelRerouteSchema = Schema.Struct({
	fromModel: Schema.String,
	reason: Schema.String,
	threadId: Schema.String,
	toModel: Schema.String,
	turnId: Schema.String,
});
const ApprovalFields = {
	itemId: Schema.String,
	threadId: Schema.String,
	turnId: Schema.NullOr(Schema.String),
};
const CommandApprovalSchema = Schema.Struct({
	...ApprovalFields,
	command: Schema.optional(Schema.String),
	cwd: Schema.optional(Schema.String),
	reason: Schema.optional(Schema.NullOr(Schema.String)),
});
const FileChangeApprovalSchema = Schema.Struct({
	...ApprovalFields,
	reason: Schema.optional(Schema.NullOr(Schema.String)),
});
const QuestionSchema = Schema.Struct({
	itemId: Schema.String,
	questions: Schema.Array(
		Schema.Struct({
			header: Schema.String,
			id: Schema.String,
			question: Schema.String,
		}),
	),
	threadId: Schema.String,
	turnId: Schema.String,
});

function make_raw(input: CodexNormalizationInput): EngineRawProvenance {
	return {
		engine_id: "codex",
		frame: input.payload,
		frame_sequence: input.frame_sequence,
		...(input.id === undefined ? {} : { native_id: input.id }),
		native_method: input.method,
		protocol_version: input.protocol_version,
		raw_frame_base64: input.raw_frame_base64,
		transport: input.transport,
	};
}

function make_base(input: CodexNormalizationInput, suffix?: string) {
	return {
		artisan_run_id: input.artisan_run_id,
		observation_id: [input.artisan_run_id, "native", String(input.frame_sequence), suffix]
			.filter((part) => part !== undefined)
			.join(":"),
		raw: make_raw(input),
	};
}

function native_action(
	input: CodexNormalizationInput,
	detail?: string,
): EngineNativeActionObservation {
	return {
		...make_base(input),
		_tag: "native_action",
		action: input.method,
		...(detail ? { detail } : {}),
		sequence: 0,
	};
}

function decode_known<S extends Schema.Constraint>(
	input: CodexNormalizationInput,
	schema: S,
	map: (value: S["Type"]) => ReadonlyArray<EngineObservation>,
): Effect.Effect<ReadonlyArray<EngineObservation>, never, S["DecodingServices"]> {
	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "preserve" })(input.payload).pipe(
		Effect.map(map),
		Effect.catch(() => Effect.succeed([native_action(input, "Malformed known Codex payload")])),
	);
}

function turn_state(status: "completed" | "failed" | "inProgress" | "interrupted") {
	return status === "completed"
		? "completed"
		: status === "interrupted"
			? "cancelled"
			: status === "failed"
				? "failed"
				: "started";
}

/**
 * Validates the minimum supported Codex fields needed for provider-neutral events while
 * preserving provider additions in raw provenance. Unknown or malformed future
 * frames remain observable as native actions.
 *
 * @since 0.3.0
 * @param input - One ordered app-server notification or server request.
 * @returns Canonical observations retaining the original raw frame provenance.
 */
export function normalise_codex_notification(
	input: CodexNormalizationInput,
): Effect.Effect<ReadonlyArray<EngineObservation>> {
	const base = make_base(input);

	switch (input.method) {
		case "thread/started":
			return decode_known(input, ThreadStartedSchema, (value) => [
				{
					...base,
					_tag: "run_state",
					sequence: 0,
					state:
						value.thread.status.type === "active" &&
						value.thread.status.activeFlags.length === 0
							? "running"
							: "waiting",
				} satisfies EngineRunStateObservation,
			]);
		case "thread/status/changed":
			return decode_known(input, ThreadStatusSchemaEnvelope, (value) => [
				{
					...base,
					_tag: "run_state",
					sequence: 0,
					state:
						value.status.type === "active" && value.status.activeFlags.length === 0
							? "running"
							: "waiting",
				} satisfies EngineRunStateObservation,
			]);
		case "turn/started":
		case "turn/completed":
			return decode_known(input, TurnEnvelopeSchema, (value) => [
				{
					...base,
					_tag: "turn_state",
					sequence: 0,
					state: turn_state(value.turn.status),
					turn_id: value.turn.id,
				} satisfies EngineTurnStateObservation,
			]);
		case "item/agentMessage/delta":
			return decode_known(input, AgentDeltaSchema, (value) => [
				{
					...base,
					_tag: "agent_message_delta",
					delta: value.delta,
					item_id: value.itemId,
					phase: "unspecified",
					sequence: 0,
					turn_id: value.turnId,
				} satisfies EngineAgentMessageDeltaObservation,
			]);
		case "item/reasoning/summaryTextDelta":
			return decode_known(input, ReasoningSummaryDeltaSchema, (value) => [
				{
					...base,
					_tag: "reasoning_summary_delta",
					delta: value.delta,
					item_id: value.itemId,
					sequence: 0,
					summary_index: value.summaryIndex,
					turn_id: value.turnId,
				} satisfies EngineReasoningSummaryDeltaObservation,
			]);
		case "item/reasoning/textDelta":
			return decode_known(input, PrivateReasoningDeltaSchema, () => [
				native_action(input, "Private reasoning text retained only in raw provenance"),
			]);
		case "turn/plan/updated":
			return decode_known(input, PlanSchema, (value) => [
				{
					...base,
					_tag: "plan",
					entries: value.plan.map((entry, index) => ({
						id: `${value.turnId}:plan:${index}`,
						status: entry.status === "inProgress" ? "in_progress" : entry.status,
						text: entry.step,
					})),
					sequence: 0,
					turn_id: value.turnId,
				} satisfies EnginePlanObservation,
			]);
		case "item/commandExecution/outputDelta":
			return decode_known(input, CommandOutputSchema, (value) => [
				{
					...base,
					_tag: "terminal_activity",
					activity_id: value.itemId,
					output: value.delta,
					sequence: 0,
					state: "output",
				} satisfies EngineTerminalActivityObservation,
			]);
		case "item/fileChange/outputDelta":
			return decode_known(input, FileOutputSchema, () => [
				native_action(input, "Received deprecated file-change output delta"),
			]);
		case "item/mcpToolCall/progress":
			return decode_known(input, McpProgressSchema, (value) => [
				{
					...base,
					_tag: "tool",
					action: "progress",
					detail: value.message,
					sequence: 0,
					tool_id: value.itemId,
					tool_name: "mcp",
				} satisfies EngineToolObservation,
			]);
		case "thread/tokenUsage/updated":
			return decode_known(input, UsageSchema, (value) => [
				{
					...base,
					_tag: "usage",
					basis: "cumulative",
					...(value.tokenUsage.total.inputTokens === undefined
						? {}
						: { input_tokens: value.tokenUsage.total.inputTokens }),
					...(value.tokenUsage.total.outputTokens === undefined
						? {}
						: { output_tokens: value.tokenUsage.total.outputTokens }),
					sequence: 0,
					turn_id: value.turnId,
				} satisfies EngineUsageObservation,
			]);
		case "thread/compacted":
			return decode_known(input, CompactionSchema, () => [
				{
					...base,
					_tag: "compaction",
					sequence: 0,
					state: "completed",
				} satisfies EngineCompactionObservation,
			]);
		case "error":
			return decode_known(input, ErrorSchema, (value) => [
				{
					...base,
					_tag: "retry",
					attempt_state: value.willRetry ? "retrying" : "terminal",
					message: value.error.message,
					sequence: 0,
					turn_id: value.turnId,
					will_retry: value.willRetry,
				} satisfies EngineRetryObservation,
			]);
		case "warning":
			return decode_known(input, WarningSchema, (value) => [
				{
					...base,
					_tag: "protocol_diagnostic",
					level: "warning",
					message: value.message,
					sequence: 0,
				} satisfies EngineProtocolDiagnosticObservation,
			]);
		case "model/rerouted":
			return decode_known(input, ModelRerouteSchema, (value) => [
				native_action(input, `${value.fromModel} -> ${value.toModel}: ${value.reason}`),
			]);
		case "item/commandExecution/requestApproval":
			return decode_known(input, CommandApprovalSchema, (value) => {
				const reason = value.reason?.trim() || undefined;
				const command = value.command?.trim() || undefined;
				const cwd = value.cwd?.trim() || undefined;

				return [
					{
						...base,
						_tag: "approval",
						approval_id: String(input.id ?? value.itemId),
						description: reason ?? "Run a command",
						request: {
							kind: "command",
							...(command === undefined ? {} : { command }),
							...(cwd === undefined ? {} : { cwd }),
							...(reason === undefined ? {} : { reason }),
						},
						sequence: 0,
						state: "requested",
					} satisfies EngineApprovalObservation,
				];
			});
		case "item/fileChange/requestApproval":
			return decode_known(input, FileChangeApprovalSchema, (value) => {
				const reason = value.reason?.trim() || undefined;

				return [
					{
						...base,
						_tag: "approval",
						approval_id: String(input.id ?? value.itemId),
						description: reason ?? "Apply file changes",
						request: {
							kind: "file_change",
							...(reason === undefined ? {} : { reason }),
						},
						sequence: 0,
						state: "requested",
					} satisfies EngineApprovalObservation,
				];
			});
		case "item/tool/requestUserInput":
			return decode_known(input, QuestionSchema, (value) =>
				value.questions.map(
					(question) =>
						({
							...make_base(input, `question:${question.id}`),
							_tag: "question",
							question_id: question.id,
							sequence: 0,
							state: "requested",
							text: question.question,
						}) satisfies EngineQuestionObservation,
				),
			);
		case "item/started":
		case "item/completed":
			return decode_known(input, ItemEnvelopeSchema, (value) => {
				const started = input.method === "item/started";

				switch (value.item.type) {
					case "agentMessage":
						return started
							? [native_action(input, "Started agent message item")]
							: [
									{
										...base,
										_tag: "agent_message_completed",
										item_id: value.item.id,
										message: value.item.text,
										phase: assistant_message_phase(value.item.phase),
										sequence: 0,
										turn_id: value.turnId,
									} satisfies EngineAgentMessageCompletedObservation,
								];
					case "plan":
						return [
							{
								...base,
								_tag: "plan",
								entries: [
									{
										id: value.item.id,
										status: started ? "in_progress" : "completed",
										text: value.item.text,
									},
								],
								sequence: 0,
								turn_id: value.turnId,
							} satisfies EnginePlanObservation,
						];
					case "commandExecution":
						return [
							{
								...base,
								_tag: "terminal_activity",
								activity_id: value.item.id,
								command: value.item.command,
								...(value.item.exitCode === null
									? {}
									: { exit_code: value.item.exitCode }),
								...(value.item.aggregatedOutput === null
									? {}
									: { output: value.item.aggregatedOutput }),
								sequence: 0,
								state: started
									? "started"
									: value.item.status === "failed" ||
										  value.item.status === "declined"
										? "failed"
										: "completed",
							} satisfies EngineTerminalActivityObservation,
						];
					case "fileChange":
						if (started || value.item.status !== "completed") {
							return [native_action(input, `File change ${value.item.status}`)];
						}

						return value.item.changes.map(
							(change, index) =>
								({
									...make_base(input, `file:${index}`),
									_tag: "file" as const,
									action:
										change.kind.type === "add"
											? ("created" as const)
											: change.kind.type === "delete"
												? ("deleted" as const)
												: ("modified" as const),
									path: change.path,
									sequence: 0,
								}) satisfies EngineFileObservation,
						);
					case "mcpToolCall":
					case "dynamicToolCall":
						return [
							{
								...base,
								_tag: "tool",
								action: started
									? "started"
									: value.item.status === "failed"
										? "failed"
										: "completed",
								sequence: 0,
								tool_id: value.item.id,
								tool_name:
									value.item.type === "mcpToolCall"
										? `${value.item.server}/${value.item.tool}`
										: `${value.item.namespace ?? "dynamic"}/${value.item.tool}`,
							} satisfies EngineToolObservation,
						];
					case "webSearch":
						return [
							{
								...base,
								_tag: "search",
								query: value.item.query,
								sequence: 0,
								state: started ? "started" : "completed",
							} satisfies EngineSearchObservation,
						];
					case "subAgentActivity":
						return [
							native_action(input, `Subagent activity: ${value.item.agentThreadId}`),
						];
					case "contextCompaction":
						return [
							{
								...base,
								_tag: "compaction",
								compaction_id: value.item.id,
								sequence: 0,
								state: started ? "started" : "completed",
							} satisfies EngineCompactionObservation,
						];
				}
			});
		case "thread/closed":
			return Effect.succeed([native_action(input, "Native Codex thread closed")]);
		default:
			return Effect.succeed([native_action(input, "Unknown Codex method")]);
	}
}
