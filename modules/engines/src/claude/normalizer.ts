import { Option, Schema } from "effect";

import { TokenCount } from "../engine";

import type {
	EngineAgentMessageCompletedObservation,
	EngineAgentMessageDeltaObservation,
	EngineCompactionObservation,
	EngineFileObservation,
	EngineObservation,
	EngineRawProvenance,
	EngineReasoningSummaryCompletedObservation,
	EngineReasoningSummaryDeltaObservation,
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
	/**
	 * The native message id most recently announced by `message_start`. Delta
	 * frames never carry it themselves, so the caller threads it through to keep
	 * a streamed message and its completion on one conversation item.
	 */
	readonly stream_message_id?: string;
	readonly turn_id: string;
}

const EventSchema = Schema.Struct({ type: Schema.String });
const InitSchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("init"),
	session_id: Schema.String,
	tools: Schema.Array(Schema.Unknown),
});
const RetrySchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("api_retry"),
});
const CompactBoundarySchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("compact_boundary"),
	uuid: Schema.NonEmptyString,
	compactMetadata: Schema.Struct({
		trigger: Schema.Literals(["manual", "auto"]),
	}),
});
/** Names the `system` bookkeeping subtypes the CLI emits around every turn. */
const SystemSubtypeSchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.String,
});
const DeltaSchema = Schema.Struct({
	type: Schema.Literal("stream_event"),
	event: Schema.Struct({
		type: Schema.Literal("content_block_delta"),
		delta: Schema.Struct({ type: Schema.Literal("text_delta"), text: Schema.String }),
	}),
});
/**
 * Claude streams private reasoning as `thinking_delta` blocks. Only the
 * provider-authored summary text travels onward; the canonical reasoning
 * observation is what the renderer shows while a turn is still thinking.
 */
const ThinkingDeltaSchema = Schema.Struct({
	type: Schema.Literal("stream_event"),
	event: Schema.Struct({
		type: Schema.Literal("content_block_delta"),
		index: Schema.optional(Schema.Number),
		delta: Schema.Struct({
			type: Schema.Literal("thinking_delta"),
			thinking: Schema.String,
		}),
	}),
});
/** Announces the native message id every subsequent delta belongs to. */
const MessageStartSchema = Schema.Struct({
	type: Schema.Literal("stream_event"),
	event: Schema.Struct({
		type: Schema.Literal("message_start"),
		message: Schema.Struct({ id: Schema.String }),
	}),
});
/** Names the stream lifecycle frames that carry no public content of their own. */
const StreamLifecycleSchema = Schema.Struct({
	type: Schema.Literal("stream_event"),
	event: Schema.Struct({ type: Schema.String }),
});
/**
 * Content-block deltas that carry neither assistant text nor reasoning:
 * thinking-block signatures and streamed tool-input JSON, whose settled forms
 * arrive in the assistant frame.
 */
const OpaqueContentDeltaSchema = Schema.Struct({
	type: Schema.Literal("stream_event"),
	event: Schema.Struct({
		type: Schema.Literal("content_block_delta"),
		delta: Schema.Struct({ type: Schema.String }),
	}),
});
const RateLimitSchema = Schema.Struct({
	type: Schema.Literal("rate_limit_event"),
	rate_limit_info: Schema.Struct({ status: Schema.optional(Schema.String) }),
});
const TextSchema = Schema.Struct({ type: Schema.Literal("text"), text: Schema.String });
const ToolUseSchema = Schema.Struct({
	type: Schema.Literal("tool_use"),
	id: Schema.String,
	name: Schema.String,
	input: Schema.Unknown,
});
const ThinkingSchema = Schema.Struct({ type: Schema.Literal("thinking"), thinking: Schema.String });
const UsageSchema = Schema.Struct({
	cache_creation_input_tokens: Schema.optional(TokenCount),
	cache_read_input_tokens: Schema.optional(TokenCount),
	input_tokens: Schema.optional(TokenCount),
	output_tokens: Schema.optional(TokenCount),
});
const AssistantSchema = Schema.Struct({
	type: Schema.Literal("assistant"),
	message: Schema.Struct({
		content: Schema.Array(Schema.Unknown),
		id: Schema.optional(Schema.String),
		usage: Schema.optional(UsageSchema),
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
const ResultSchema = Schema.Struct({
	type: Schema.Literal("result"),
	subtype: Schema.String,
	session_id: Schema.optional(Schema.String),
	is_error: Schema.optional(Schema.Boolean),
	usage: Schema.optional(UsageSchema),
	permission_denials: Schema.optional(Schema.Array(Schema.Unknown)),
	errors: Schema.optional(Schema.Array(Schema.String)),
});
/**
 * The CLI's terminal frame omits `type` in the current protocol, so the
 * result is recognized by its own terminal fields instead of an envelope tag.
 */
const TerminalResultSchema = Schema.Struct({
	is_error: Schema.Boolean,
	stop_reason: Schema.optional(Schema.NullOr(Schema.String)),
	session_id: Schema.optional(Schema.String),
	usage: Schema.optional(UsageSchema),
	permission_denials: Schema.optional(Schema.Array(Schema.Unknown)),
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
 *
 * A streamed message and its completion must resolve to the same item, or the
 * completion upserts a second copy beside the streamed one. Only `message_start`
 * discloses the native id, so the caller threads it forward and both paths
 * prefer it; a run-stable synthesized id remains the fallback when the CLI
 * streams content without ever announcing a message.
 */
function message_item_id(input: ClaudeNormalizationInput, native_id: string | undefined) {
	return native_id ?? input.stream_message_id ?? `claude:${input.artisan_run_id}:message`;
}

/**
 * Reads the native message id announced by a `message_start` frame so the
 * caller can correlate the deltas that follow it.
 *
 * @since 0.7.0
 */
export function read_claude_stream_message_id(payload: unknown): string | undefined {
	return decode(MessageStartSchema, payload)?.event.message.id;
}

/**
 * Names the frames that exist purely for CLI bookkeeping. Every frame is
 * already retained verbatim as raw provenance, so re-emitting these as
 * canonical observations would only bury real diagnostics in noise.
 */
const silent_system_subtypes = new Set([
	"hook_started",
	"hook_response",
	"status",
	"thinking_tokens",
]);
const silent_stream_events = new Set([
	"message_start",
	"message_delta",
	"message_stop",
	"content_block_start",
	"content_block_stop",
	"ping",
]);

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
		...(usage.cache_read_input_tokens === undefined
			? {}
			: { cached_input_tokens: usage.cache_read_input_tokens }),
		...(usage.input_tokens === undefined ? {} : { input_tokens: usage.input_tokens }),
		...(usage.output_tokens === undefined ? {} : { output_tokens: usage.output_tokens }),
		turn_id: input.turn_id,
	};
}

/**
 * Measures the tokens occupying Claude's context window from one assistant
 * frame's per-response usage: the response's input plus the cache reads and
 * writes that carried the prior conversation. The terminal `result.usage` is
 * unsuitable for this gauge because it accumulates input across every model
 * call in the turn, re-counting the context each call resent.
 */
function context_usage_observation(
	input: ClaudeNormalizationInput,
	usage: Schema.Schema.Type<typeof UsageSchema>,
): EngineUsageObservation | undefined {
	if (usage.input_tokens === undefined) return undefined;
	return {
		...make_base(input, "assistant.usage", "usage"),
		_tag: "usage",
		basis: "cumulative",
		context_tokens:
			usage.input_tokens +
			(usage.cache_creation_input_tokens ?? 0) +
			(usage.cache_read_input_tokens ?? 0),
		turn_id: input.turn_id,
	};
}

/**
 * Closes the reasoning item a buffered `assistant` frame's `thinking` block(s)
 * belong to. Constructed with the same item id the thinking-delta path uses
 * (native message id when known, else the threaded `stream_message_id`, else
 * the run-stable fallback) so the completion always resolves onto the item
 * the deltas opened, even when the provider streamed no delta text at all.
 */
function reasoning_summary_completed_observation(
	input: ClaudeNormalizationInput,
	native_message_id: string | undefined,
): EngineReasoningSummaryCompletedObservation {
	return {
		...make_base(input, "assistant.thinking"),
		_tag: "reasoning_summary_completed",
		item_id: `${message_item_id(input, native_message_id)}:reasoning`,
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
	const compact_boundary = decode(CompactBoundarySchema, input.payload);
	if (compact_boundary !== undefined) {
		const base = make_base(input, "system.compact_boundary", "compact_boundary");
		return [
			{
				...base,
				_tag: "compaction",
				compaction_id: compact_boundary.uuid,
				raw: {
					...base.raw,
					native_id: compact_boundary.uuid,
					native_method: "system.compact_boundary",
				},
				state: "completed",
			} satisfies EngineCompactionObservation,
		];
	}

	/** Bookkeeping frames stay in raw provenance only. */
	const system_frame = decode(SystemSubtypeSchema, input.payload);
	if (system_frame !== undefined && silent_system_subtypes.has(system_frame.subtype)) return [];

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

	const thinking_delta = decode(ThinkingDeltaSchema, input.payload);
	if (thinking_delta !== undefined)
		return [
			{
				...make_base(input, "stream_event.thinking_delta"),
				_tag: "reasoning_summary_delta",
				delta: thinking_delta.event.delta.thinking,
				item_id: `${message_item_id(input, undefined)}:reasoning`,
				summary_index: thinking_delta.event.index ?? 0,
				turn_id: input.turn_id,
			} satisfies EngineReasoningSummaryDeltaObservation,
		];

	const stream_lifecycle = decode(StreamLifecycleSchema, input.payload);
	if (stream_lifecycle !== undefined && silent_stream_events.has(stream_lifecycle.event.type))
		return [];
	if (decode(OpaqueContentDeltaSchema, input.payload) !== undefined) return [];

	const rate_limit = decode(RateLimitSchema, input.payload);
	if (rate_limit !== undefined)
		return rate_limit.rate_limit_info.status === "allowed"
			? []
			: [
					native_action(
						input,
						`Claude rate limit ${rate_limit.rate_limit_info.status ?? "status unknown"}`,
					),
				];

	const assistant = decode(AssistantSchema, input.payload);
	if (assistant !== undefined) {
		const text_parts: Array<string> = [];
		const observations: Array<EngineObservation> = [];
		const has_thinking = assistant.message.content.some(
			(item) => decode(ThinkingSchema, item) !== undefined,
		);
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
		/**
		 * A buffered `thinking` block (including Sonnet 5's suppressed-display
		 * shape, whose `thinking` text is empty) is the settled form of a
		 * reasoning phase that may never have streamed a delta. It closes the
		 * item the delta path opened, ordered first since thinking blocks
		 * precede the public content that follows them in the same frame.
		 */
		if (has_thinking)
			observations.unshift(
				reasoning_summary_completed_observation(input, assistant.message.id),
			);
		if (assistant.message.usage !== undefined) {
			const context_usage = context_usage_observation(input, assistant.message.usage);
			if (context_usage !== undefined) observations.push(context_usage);
		}
		if (observations.length > 0) return observations;
		if (assistant.error !== undefined) return [native_action(input, assistant.error)];
		return [native_action(input, "Assistant event without public content")];
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

	/**
	 * The current CLI emits its terminal summary without an envelope `type`,
	 * so it is recognized by its own terminal fields. Usage is the only
	 * canonical content; a failure surfaces its stop reason.
	 */
	const terminal = decode(TerminalResultSchema, input.payload);
	if (terminal !== undefined) {
		const observations: Array<EngineObservation> = [];
		if (terminal.usage !== undefined)
			observations.push(usage_observation(input, terminal.usage));
		if (terminal.permission_denials !== undefined && terminal.permission_denials.length > 0)
			observations.push(
				native_action(
					input,
					"Claude permission denial retained without approval semantics",
				),
			);
		if (terminal.is_error)
			observations.push(
				native_action(
					input,
					`Claude run failed: ${terminal.stop_reason ?? "no stop reason reported"}`,
				),
			);
		return observations;
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
