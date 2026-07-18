import { Effect, Schema } from "effect";

import { TokenCount } from "../engine";

import {
	type EngineObservation,
	type EngineObservationBase,
	type EngineRawProvenance,
	type EngineAgentMessageCompletedObservation,
	type EngineFileObservation,
	type EngineNativeActionObservation,
	type EnginePlanObservation,
	type EngineProtocolDiagnosticObservation,
	type EngineRunStateObservation,
	type EngineSearchObservation,
	type EngineTerminalActivityObservation,
	type EngineToolObservation,
	type EngineTurnStateObservation,
	type EngineUsageObservation,
} from "../engine";
import { codex_exec_protocol_version, codex_exec_transport } from "./internal/codex-exec-contract";

/** Supplies one decoded `codex exec --json` event with byte-faithful provenance. */
export interface CodexExecNormalizationInput {
	readonly artisan_run_id: string;
	readonly frame_sequence: number;
	readonly payload: unknown;
	readonly raw_frame_base64: string;
	readonly turn_id: string;
}

/** Names the semantic run outcome signaled by one decoded exec event. */
export type CodexExecSemanticOutcome = "continue" | "failed";

const exec_event_schema = Schema.Struct({ type: Schema.String });
const thread_started_schema = Schema.Struct({
	thread_id: Schema.String,
	type: Schema.Literal("thread.started"),
});
const turn_completed_schema = Schema.Struct({
	type: Schema.Literal("turn.completed"),
	usage: Schema.optional(
		Schema.Struct({
			cached_input_tokens: Schema.optional(TokenCount),
			input_tokens: Schema.optional(TokenCount),
			output_tokens: Schema.optional(TokenCount),
			reasoning_output_tokens: Schema.optional(TokenCount),
		}),
	),
});
const error_event_schema = Schema.Struct({
	message: Schema.optional(Schema.String),
	type: Schema.Literal("error"),
});
const item_envelope_schema = Schema.Struct({
	item: Schema.Struct({ id: Schema.String, type: Schema.String }),
	type: Schema.Literals(["item.started", "item.updated", "item.completed"]),
});
const agent_message_schema = Schema.Struct({
	item: Schema.Struct({
		id: Schema.String,
		text: Schema.String,
		type: Schema.Literal("agent_message"),
	}),
	type: Schema.Literals(["item.started", "item.updated", "item.completed"]),
});
const command_schema = Schema.Struct({
	item: Schema.Struct({
		aggregated_output: Schema.optional(Schema.String),
		command: Schema.String,
		exit_code: Schema.optional(Schema.NullOr(Schema.Number)),
		id: Schema.String,
		status: Schema.optional(Schema.String),
		type: Schema.Literal("command_execution"),
	}),
	type: Schema.Literals(["item.started", "item.updated", "item.completed"]),
});
const file_change_schema = Schema.Struct({
	item: Schema.Struct({
		changes: Schema.Array(
			Schema.Struct({
				kind: Schema.String,
				path: Schema.String,
			}),
		),
		id: Schema.String,
		type: Schema.Literal("file_change"),
	}),
	type: Schema.Literals(["item.started", "item.updated", "item.completed"]),
});
const search_schema = Schema.Struct({
	item: Schema.Struct({
		id: Schema.String,
		query: Schema.String,
		type: Schema.Literal("web_search"),
	}),
	type: Schema.Literals(["item.started", "item.updated", "item.completed"]),
});
const tool_schema = Schema.Struct({
	item: Schema.Struct({
		id: Schema.String,
		server: Schema.optional(Schema.String),
		tool: Schema.String,
		type: Schema.Literals(["mcp_tool_call", "dynamic_tool_call"]),
	}),
	type: Schema.Literals(["item.started", "item.updated", "item.completed"]),
});
const plan_schema = Schema.Struct({
	item: Schema.Struct({
		id: Schema.String,
		items: Schema.Array(
			Schema.Struct({
				completed: Schema.optional(Schema.Boolean),
				status: Schema.optional(Schema.String),
				text: Schema.String,
			}),
		),
		type: Schema.Literals(["plan", "plan_update"]),
	}),
	type: Schema.Literals(["item.started", "item.updated", "item.completed"]),
});

/** Classifies fatal top-level exec events independently from process exit status. */
export function ClassifyCodexExecSemanticOutcome(
	payload: unknown,
): Effect.Effect<CodexExecSemanticOutcome> {
	return Schema.decodeUnknownEffect(exec_event_schema, { onExcessProperty: "preserve" })(
		payload,
	).pipe(
		Effect.map((event) =>
			event.type === "turn.failed" || event.type === "error" ? "failed" : "continue",
		),
		Effect.catch(() => Effect.succeed("continue" as const)),
	);
}

function make_raw(input: CodexExecNormalizationInput, type: string): EngineRawProvenance {
	return {
		engine_id: "codex",
		frame: input.payload,
		frame_sequence: input.frame_sequence,
		native_method: type,
		protocol_version: codex_exec_protocol_version,
		raw_frame_base64: input.raw_frame_base64,
		transport: codex_exec_transport,
	};
}

function make_base(
	input: CodexExecNormalizationInput,
	type: string,
	suffix?: string,
): EngineObservationBase {
	return {
		artisan_run_id: input.artisan_run_id,
		observation_id: [input.artisan_run_id, "exec", String(input.frame_sequence), suffix]
			.filter((part) => part !== undefined)
			.join(":"),
		raw: make_raw(input, type),
		sequence: 0,
	};
}

function native_action(
	input: CodexExecNormalizationInput,
	type: string,
	detail?: string,
): EngineNativeActionObservation {
	return {
		...make_base(input, type),
		_tag: "native_action",
		action: type,
		...(detail === undefined ? {} : { detail }),
	};
}

function DecodeKnown<S extends Schema.Constraint>(
	input: CodexExecNormalizationInput,
	type: string,
	schema: S,
	map: (value: S["Type"]) => ReadonlyArray<EngineObservation>,
): Effect.Effect<ReadonlyArray<EngineObservation>, never, S["DecodingServices"]> {
	return Schema.decodeUnknownEffect(schema, { onExcessProperty: "preserve" })(input.payload).pipe(
		Effect.map(map),
		Effect.catch(() =>
			Effect.succeed([native_action(input, type, "Malformed known Codex exec event")]),
		),
	);
}

function NormaliseItem(input: CodexExecNormalizationInput, type: string, item_type: string) {
	const action =
		type === "item.started" ? "started" : type === "item.completed" ? "completed" : "progress";

	switch (item_type) {
		case "agent_message":
			return DecodeKnown(input, type, agent_message_schema, (value) =>
				value.type === "item.completed"
					? [
							{
								...make_base(input, type),
								_tag: "agent_message_completed",
								message: value.item.text,
								turn_id: input.turn_id,
							} satisfies EngineAgentMessageCompletedObservation,
						]
					: [native_action(input, type, `Agent message ${action}`)],
			);
		case "reasoning":
			return Effect.succeed([
				native_action(
					input,
					type,
					`Reasoning ${action}; text retained only in raw provenance`,
				),
			]);
		case "command_execution":
			return DecodeKnown(input, type, command_schema, (value) => [
				{
					...make_base(input, type),
					_tag: "terminal_activity",
					activity_id: value.item.id,
					command: value.item.command,
					...(value.item.exit_code === undefined || value.item.exit_code === null
						? {}
						: { exit_code: value.item.exit_code }),
					...(value.item.aggregated_output === undefined
						? {}
						: { output: value.item.aggregated_output }),
					state:
						value.type === "item.started"
							? "started"
							: value.type === "item.updated"
								? "output"
								: value.item.status === "failed" ||
									  (value.item.exit_code ?? 0) !== 0
									? "failed"
									: "completed",
				} satisfies EngineTerminalActivityObservation,
			]);
		case "file_change":
			return DecodeKnown(input, type, file_change_schema, (value) =>
				value.type !== "item.completed"
					? [native_action(input, type, `File change ${action}`)]
					: value.item.changes.map(
							(change, index) =>
								({
									...make_base(input, type, `file:${index}`),
									_tag: "file",
									action:
										change.kind === "add" || change.kind === "create"
											? "created"
											: change.kind === "delete"
												? "deleted"
												: "modified",
									path: change.path,
								}) satisfies EngineFileObservation,
						),
			);
		case "web_search":
			return DecodeKnown(input, type, search_schema, (value) => [
				{
					...make_base(input, type),
					_tag: "search",
					query: value.item.query,
					state: value.type === "item.completed" ? "completed" : "started",
				} satisfies EngineSearchObservation,
			]);
		case "mcp_tool_call":
		case "dynamic_tool_call":
			return DecodeKnown(input, type, tool_schema, (value) => [
				{
					...make_base(input, type),
					_tag: "tool",
					action,
					tool_id: value.item.id,
					tool_name: `${value.item.server ?? "dynamic"}/${value.item.tool}`,
				} satisfies EngineToolObservation,
			]);
		case "plan":
		case "plan_update":
			return DecodeKnown(input, type, plan_schema, (value) => [
				{
					...make_base(input, type),
					_tag: "plan",
					entries: value.item.items.map((entry, index) => ({
						id: `${value.item.id}:${index}`,
						status:
							entry.completed === true || entry.status === "completed"
								? "completed"
								: entry.status === "in_progress"
									? "in_progress"
									: "pending",
						text: entry.text,
					})),
					turn_id: input.turn_id,
				} satisfies EnginePlanObservation,
			]);
		default:
			return Effect.succeed([
				native_action(input, type, `Unknown exec item type: ${item_type}`),
			]);
	}
}

/** Normalizes one valid `codex exec --json` event without discarding unknown events. */
export function NormaliseCodexExecEvent(
	input: CodexExecNormalizationInput,
): Effect.Effect<ReadonlyArray<EngineObservation>> {
	return Schema.decodeUnknownEffect(exec_event_schema, { onExcessProperty: "preserve" })(
		input.payload,
	).pipe(
		Effect.flatMap((event) => {
			const type = event.type;

			switch (type) {
				case "thread.started":
					return DecodeKnown(input, type, thread_started_schema, () => [
						{
							...make_base(input, type),
							_tag: "run_state",
							state: "running",
						} satisfies EngineRunStateObservation,
					]);
				case "turn.started":
					return Effect.succeed([
						{
							...make_base(input, type),
							_tag: "turn_state",
							state: "started",
							turn_id: input.turn_id,
						} satisfies EngineTurnStateObservation,
					]);
				case "turn.completed":
					return DecodeKnown(input, type, turn_completed_schema, (value) => [
						{
							...make_base(input, type),
							_tag: "turn_state",
							state: "completed",
							turn_id: input.turn_id,
						} satisfies EngineTurnStateObservation,
						...(value.usage === undefined
							? []
							: [
									{
										...make_base(input, type, "usage"),
										_tag: "usage" as const,
										basis: "delta",
										...(value.usage.input_tokens === undefined
											? {}
											: { input_tokens: value.usage.input_tokens }),
										...(value.usage.output_tokens === undefined
											? {}
											: { output_tokens: value.usage.output_tokens }),
										turn_id: input.turn_id,
									} satisfies EngineUsageObservation,
								]),
					]);
				case "turn.failed":
					return Effect.succeed([
						{
							...make_base(input, type),
							_tag: "turn_state",
							state: "failed",
							turn_id: input.turn_id,
						} satisfies EngineTurnStateObservation,
					]);
				case "error":
					return DecodeKnown(input, type, error_event_schema, (value) => [
						{
							...make_base(input, type),
							_tag: "protocol_diagnostic",
							level: "error",
							message: value.message ?? "Codex exec reported an error",
						} satisfies EngineProtocolDiagnosticObservation,
					]);
				case "item.started":
				case "item.updated":
				case "item.completed":
					return Schema.decodeUnknownEffect(item_envelope_schema, {
						onExcessProperty: "preserve",
					})(input.payload).pipe(
						Effect.flatMap((value) => NormaliseItem(input, type, value.item.type)),
						Effect.catch(() =>
							Effect.succeed([
								native_action(input, type, "Malformed known Codex exec event"),
							]),
						),
					);
				default:
					return Effect.succeed([native_action(input, type, "Unknown Codex exec event")]);
			}
		}),
		Effect.catch(() =>
			Effect.succeed([
				native_action(input, "invalid", "Exec event is not an object with a type"),
			]),
		),
	);
}
