import { Option, Schema } from "effect";

import { TokenCount } from "../engine";

import type {
	EngineAgentMessageCompletedObservation,
	EngineAgentMessageDeltaObservation,
	EngineFileObservation,
	EngineObservation,
	EngineRawProvenance,
	EngineSearchObservation,
	EngineTerminalActivityObservation,
	EngineToolObservation,
	EngineUsageObservation,
} from "../engine";

/** Supplies one decoded Claude stream-json event to the canonical normalizer. @since 0.6.0 */
export interface ClaudeNormalizationInput {
	readonly artisan_run_id: string;
	readonly frame_sequence: number;
	readonly payload: unknown;
	readonly raw_frame_base64: string;
	readonly turn_id: string;
}

const EventSchema = Schema.Struct({ type: Schema.String });
const InitSchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("init"),
	session_id: Schema.String,
	model: Schema.String,
	tools: Schema.Array(Schema.Unknown),
	permissionMode: Schema.String,
});
const RetrySchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("api_retry"),
});
const DeltaSchema = Schema.Struct({
	type: Schema.Literal("stream_event"),
	event: Schema.Struct({
		type: Schema.Literal("content_block_delta"),
		delta: Schema.Struct({ type: Schema.Literal("text_delta"), text: Schema.String }),
	}),
});
const TextSchema = Schema.Struct({ type: Schema.Literal("text"), text: Schema.String });
const ToolUseSchema = Schema.Struct({
	type: Schema.Literal("tool_use"),
	id: Schema.String,
	name: Schema.String,
	input: Schema.Unknown,
});
const ThinkingSchema = Schema.Struct({ type: Schema.Literal("thinking"), thinking: Schema.String });
const AssistantSchema = Schema.Struct({
	type: Schema.Literal("assistant"),
	message: Schema.Struct({
		content: Schema.Array(Schema.Unknown),
		id: Schema.optional(Schema.String),
	}),
	error: Schema.optional(Schema.String),
});
const AssistantErrorSchema = Schema.Struct({
	type: Schema.Literal("assistant"),
	error: Schema.String,
});
const ToolResultSchema = Schema.Struct({
	type: Schema.Literal("tool_result"),
	tool_use_id: Schema.String,
	is_error: Schema.optional(Schema.Boolean),
	content: Schema.Unknown,
});
const UserSchema = Schema.Struct({
	type: Schema.Literal("user"),
	message: Schema.Struct({ content: Schema.Array(Schema.Unknown) }),
});
const UsageSchema = Schema.Struct({
	input_tokens: Schema.optional(TokenCount),
	output_tokens: Schema.optional(TokenCount),
});
const ResultSchema = Schema.Struct({
	type: Schema.Literal("result"),
	subtype: Schema.String,
	session_id: Schema.optional(Schema.String),
	is_error: Schema.optional(Schema.Boolean),
	usage: Schema.optional(UsageSchema),
	permission_denials: Schema.optional(Schema.Array(Schema.Unknown)),
	errors: Schema.optional(Schema.Array(Schema.String)),
});

type DecodedSchema<S extends Schema.ConstraintDecoder<unknown>> = S["Type"] | undefined;

function decode<S extends Schema.ConstraintDecoder<unknown>>(
	schema: S,
	value: unknown,
): DecodedSchema<S> {
	const result = Schema.decodeUnknownOption(schema, { onExcessProperty: "preserve" })(value);

	return Option.isSome(result) ? result.value : undefined;
}

/** Classifies every documented Claude fatal event without exposing private reasoning. @since 0.6.0 */
export function classify_claude_semantic_failure(payload: unknown) {
	const event = decode(EventSchema, payload);
	const result = decode(ResultSchema, payload);
	const assistant_error = decode(AssistantErrorSchema, payload);

	return (
		event?.type === "error" ||
		assistant_error !== undefined ||
		(result !== undefined && (result.subtype !== "success" || result.is_error === true))
	);
}

/** Reads the provider session identity from a validated Claude event envelope. @since 0.6.0 */
export function read_claude_session_id(payload: unknown): string | undefined {
	const session_event = decode(Schema.Struct({ session_id: Schema.String }), payload);
	return session_event?.session_id;
}

/** Identifies the required Claude initialization event after schema validation. @since 0.6.0 */
export function is_claude_init_event(payload: unknown) {
	return decode(InitSchema, payload) !== undefined;
}

/**
 * Names the assistant message item extended or completed by one observation.
 * Claude stream-json delta frames never disclose their native message id, so a
 * run-stable synthesized id keeps deltas correlated without inventing
 * per-message identity; assistant completions prefer the native `message.id`
 * whenever the CLI discloses one.
 */
function message_item_id(input: ClaudeNormalizationInput, native_id: string | undefined) {
	return native_id ?? `claude:${input.artisan_run_id}:message`;
}

function make_base(input: ClaudeNormalizationInput, native_method: string, suffix?: string) {
	const raw: EngineRawProvenance = {
		engine_id: "claude",
		frame: input.payload,
		frame_sequence: input.frame_sequence,
		native_method,
		protocol_version: "claude-stream-json-v1",
		raw_frame_base64: input.raw_frame_base64,
		transport: "claude-cli-stream-json",
	};

	return {
		artisan_run_id: input.artisan_run_id,
		observation_id: [input.artisan_run_id, "claude", String(input.frame_sequence), suffix]
			.filter((part) => part !== undefined)
			.join(":"),
		raw,
		sequence: 0,
	};
}

function native_action(input: ClaudeNormalizationInput, detail: string) {
	return {
		...make_base(input, "unknown"),
		_tag: "native_action",
		action: "claude_event",
		detail,
	} satisfies EngineObservation;
}

/**
 * Maps Claude's terminal `result.usage` totals onto the canonical usage
 * observation. The CLI reports per-run totals exactly once, so the basis is
 * `cumulative` rather than a per-message delta.
 */
function usage_observation(
	input: ClaudeNormalizationInput,
	usage: Schema.Schema.Type<typeof UsageSchema>,
): EngineUsageObservation {
	return {
		...make_base(input, "result.usage"),
		_tag: "usage",
		basis: "cumulative",
		...(usage.input_tokens === undefined ? {} : { input_tokens: usage.input_tokens }),
		...(usage.output_tokens === undefined ? {} : { output_tokens: usage.output_tokens }),
		turn_id: input.turn_id,
	};
}

function tool_observation(
	input: ClaudeNormalizationInput,
	tool: Schema.Schema.Type<typeof ToolUseSchema>,
	action: EngineToolObservation["action"] = "started",
): EngineObservation {
	const input_value = decode(
		Schema.Struct({
			command: Schema.optional(Schema.String),
			file_path: Schema.optional(Schema.String),
			path: Schema.optional(Schema.String),
			query: Schema.optional(Schema.String),
			url: Schema.optional(Schema.String),
		}),
		tool.input,
	);
	const name = tool.name;

	if (name === "Bash") {
		return {
			...make_base(input, "assistant.tool_use"),
			_tag: "terminal_activity",
			activity_id: tool.id,
			...(input_value?.command === undefined ? {} : { command: input_value.command }),
			state: action === "completed" ? "completed" : "started",
		} satisfies EngineTerminalActivityObservation;
	}

	if (["Edit", "Write", "NotebookEdit"].includes(name)) {
		return {
			...make_base(input, "assistant.tool_use"),
			_tag: "file",
			action: "modified",
			path: input_value?.file_path ?? input_value?.path ?? "unknown",
		} satisfies EngineFileObservation;
	}

	if (["WebSearch", "WebFetch"].includes(name)) {
		return {
			...make_base(input, "assistant.tool_use"),
			_tag: "search",
			query: input_value?.query ?? input_value?.url ?? "",
			state: action === "started" ? "started" : "completed",
		} satisfies EngineSearchObservation;
	}

	return {
		...make_base(input, "assistant.tool_use"),
		_tag: "tool",
		action,
		detail: "Claude tool input retained in raw provenance",
		tool_id: tool.id,
		tool_name: name,
	} satisfies EngineToolObservation;
}

/**
 * Normalizes documented Claude events with schema validation and raw-frame provenance.
 *
 * Assistant text is emitted with phase `unspecified` because Claude's
 * stream-json protocol never distinguishes commentary from a settled final
 * reply, and inferring a phase from message order would fabricate provider
 * intent. API retry progress remains a native action rather than a canonical
 * retry observation because the native stream discloses neither an attempt
 * lifecycle nor whether the provider will retry.
 *
 * @since 0.6.0
 */
export function normalize_claude_event(
	input: ClaudeNormalizationInput,
): ReadonlyArray<EngineObservation> {
	const init = decode(InitSchema, input.payload);
	if (init !== undefined)
		return [{ ...make_base(input, "system.init"), _tag: "run_state", state: "running" }];
	if (decode(RetrySchema, input.payload) !== undefined)
		return [native_action(input, "Claude API retry progress")];

	const delta = decode(DeltaSchema, input.payload);
	if (delta !== undefined)
		return [
			{
				...make_base(input, "stream_event.content_block_delta"),
				_tag: "agent_message_delta",
				delta: delta.event.delta.text,
				item_id: message_item_id(input, undefined),
				phase: "unspecified",
				turn_id: input.turn_id,
			} satisfies EngineAgentMessageDeltaObservation,
		];

	const assistant = decode(AssistantSchema, input.payload);
	if (assistant !== undefined) {
		const text_parts: Array<string> = [];
		const observations: Array<EngineObservation> = [];
		assistant.message.content.forEach((item, index) => {
			const text = decode(TextSchema, item);
			const tool = decode(ToolUseSchema, item);
			if (text !== undefined) text_parts.push(text.text);
			if (tool !== undefined) observations.push(tool_observation(input, tool));
			if (decode(ThinkingSchema, item) !== undefined) return;
			if (
				text === undefined &&
				tool === undefined &&
				decode(Schema.Struct({ type: Schema.String }), item) !== undefined
			)
				observations.push(
					native_action(input, `Malformed assistant content at index ${index}`),
				);
		});
		if (text_parts.length > 0)
			observations.unshift({
				...make_base(input, "assistant.text"),
				_tag: "agent_message_completed",
				item_id: message_item_id(input, assistant.message.id),
				message: text_parts.join(""),
				phase: "unspecified",
				turn_id: input.turn_id,
			} satisfies EngineAgentMessageCompletedObservation);
		return observations.length === 0
			? [native_action(input, assistant.error ?? "Assistant event without public content")]
			: observations;
	}

	const user = decode(UserSchema, input.payload);
	if (user !== undefined)
		return user.message.content.map((item, index) => {
			const result = decode(ToolResultSchema, item);
			return result === undefined
				? native_action(input, `Malformed user content at index ${index}`)
				: ({
						...make_base(input, "user.tool_result", result.tool_use_id),
						_tag: "tool",
						action: result.is_error === true ? "failed" : "completed",
						detail: "Tool result content retained in raw provenance",
						tool_id: result.tool_use_id,
						tool_name: "claude-tool",
					} satisfies EngineToolObservation);
		});

	const result = decode(ResultSchema, input.payload);
	if (result !== undefined) {
		const observations: Array<EngineObservation> = [];
		if (result.usage !== undefined) observations.push(usage_observation(input, result.usage));
		if (result.permission_denials !== undefined && result.permission_denials.length > 0)
			observations.push(
				native_action(
					input,
					"Claude permission denial retained without approval semantics",
				),
			);
		if (observations.length > 0) return observations;
		return [
			native_action(
				input,
				result.subtype === "success"
					? "Claude result"
					: `Claude result failure: ${result.subtype}`,
			),
		];
	}

	const event = decode(EventSchema, input.payload);
	return [
		native_action(
			input,
			event === undefined
				? "Malformed Claude event"
				: `Unknown Claude event type: ${event.type}`,
		),
	];
}
