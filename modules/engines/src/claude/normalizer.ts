import { Option, Schema } from "effect";

import { TokenCount } from "../engine";
import { CountPatchHunkLines, CountWrittenLines } from "../patch/unified-diff";
import {
	classify_claude_assistant_error,
	classify_claude_rate_limit,
	classify_claude_result_failure,
	classify_claude_terminal_failure,
} from "./errors";
import {
	normalize_claude_task_lifecycle,
	type ClaudeTaskNormalizationInput,
} from "./task-lifecycle";

export {
	has_claude_subagent_lifecycle_hint,
	read_claude_task_id,
	read_claude_task_started,
} from "./task-lifecycle";
export type { ClaudeTaskStarted } from "./task-lifecycle";

import type {
	EngineAgentMessageCompletedObservation,
	EngineAgentMessageDeltaObservation,
	EngineCompactionObservation,
	EngineErrorRef,
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
export interface ClaudeNormalizationInput extends ClaudeTaskNormalizationInput {
	/**
	 * The tool uses earlier assistant frames announced for this run. The CLI
	 * sends their results later in a user frame with only `tool_use_id`, so the
	 * run owner supplies this decoded correlation instead of guessing a tool
	 * family from result content.
	 */
	readonly tool_uses?: ReadonlyMap<string, ClaudeToolUse>;
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
/**
 * Kept deliberately loose: the boundary itself is the fact worth capturing,
 * and requiring any particular metadata internals is what silently dropped
 * every compaction before — the schema demanded `compactMetadata` while the
 * current CLI stream spells it `compact_metadata`.
 *
 * The envelope and its contents disagree about casing, and each spelling is
 * accepted on both sides for the same reason. A live 2.1.220 boundary carries
 * `compactMetadata: { preTokens, postTokens, durationMs }` — snake outside,
 * camel within — so declaring one convention throughout silently read every
 * token count as absent, and an absent `post_tokens` is what left the context
 * gauge showing the pre-compaction reading until the next response happened
 * to report usage.
 */
const CompactionMetadataSchema = Schema.Struct({
	trigger: Schema.optional(Schema.String),
	pre_tokens: Schema.optional(TokenCount),
	preTokens: Schema.optional(TokenCount),
	/** The tokens the summarized history occupies after the boundary. */
	post_tokens: Schema.optional(TokenCount),
	postTokens: Schema.optional(TokenCount),
	/** How long the compaction held the stream silent, which nothing else reports. */
	duration_ms: Schema.optional(Schema.Number),
	durationMs: Schema.optional(Schema.Number),
});

type ClaudeCompactionMetadata = typeof CompactionMetadataSchema.Type;

/** Reads one compaction measurement across both spellings the CLI has used. */
const compaction_post_tokens = (metadata: ClaudeCompactionMetadata | undefined) =>
	metadata?.post_tokens ?? metadata?.postTokens;

const compaction_duration_ms = (metadata: ClaudeCompactionMetadata | undefined) =>
	metadata?.duration_ms ?? metadata?.durationMs;
const CompactBoundarySchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("compact_boundary"),
	uuid: Schema.optional(Schema.NonEmptyString),
	compact_metadata: Schema.optional(CompactionMetadataSchema),
	/** The spelling one earlier protocol revision documented. */
	compactMetadata: Schema.optional(CompactionMetadataSchema),
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
 *
 * On the current CLI that text is never present. A live 2.1.220 stream opens
 * `content_block_start` with `{type: "thinking", thinking: "", signature: ""}`,
 * sends every `thinking_delta` with `thinking: ""`, and settles the block with
 * a signature and no text — the reasoning is encrypted, not summarized, so
 * there is nothing to forward. The shape is kept because an engine that does
 * send text must still be carried.
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
/**
 * The CLI's running estimate of how much thinking this turn has done.
 *
 * It is the only live evidence of reasoning Claude publishes, since the text
 * itself never arrives. `estimated_tokens` is cumulative for the turn and
 * `estimated_tokens_delta` is the step, so the cumulative value is what a
 * reader wants and a replayed frame cannot double-count it.
 */
const ThinkingTokensSchema = Schema.Struct({
	type: Schema.Literal("system"),
	subtype: Schema.Literal("thinking_tokens"),
	estimated_tokens: Schema.optional(TokenCount),
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
	rate_limit_info: Schema.Struct({
		rateLimitType: Schema.optional(Schema.String),
		resetsAt: Schema.optional(Schema.Number),
		status: Schema.optional(Schema.String),
	}),
});
const TextSchema = Schema.Struct({ type: Schema.Literal("text"), text: Schema.String });
const ToolUseSchema = Schema.Struct({
	type: Schema.Literal("tool_use"),
	id: Schema.String,
	name: Schema.String,
	input: Schema.Unknown,
});
/** A schema-decoded Claude tool invocation retained by the run that owns it. @since 0.7.0 */
export type ClaudeToolUse = Schema.Schema.Type<typeof ToolUseSchema>;
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
/**
 * The applied-edit report the CLI attaches to the turn that carries a tool
 * result. An update states its patch; a create states what it wrote and that
 * nothing stood there before, so both sides are counted from the engine's own
 * account of the edit rather than from the workspace.
 */
const ToolUseResultSchema = Schema.Struct({
	content: Schema.optional(Schema.String),
	filePath: Schema.optional(Schema.String),
	originalFile: Schema.optional(Schema.NullOr(Schema.String)),
	structuredPatch: Schema.optional(
		Schema.Array(Schema.Struct({ lines: Schema.Array(Schema.String) })),
	),
	type: Schema.optional(Schema.String),
});
const UserSchema = Schema.Struct({
	type: Schema.Literal("user"),
	message: Schema.Struct({ content: Schema.Array(Schema.Unknown) }),
	tool_use_result: Schema.optional(Schema.Unknown),
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
	const rate_limit = decode(RateLimitSchema, payload);

	return (
		event?.type === "error" ||
		assistant_error !== undefined ||
		rate_limit?.rate_limit_info.status === "rejected" ||
		rate_limit?.rate_limit_info.status === "exceeded" ||
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
 * Reads every tool invocation from one settled assistant frame. The process
 * adapter owns the resulting map for exactly one run; this normalizer only
 * consumes it when the later result frame needs the invocation's semantics.
 *
 * @since 0.7.0
 */
export function read_claude_tool_uses(payload: unknown): ReadonlyArray<ClaudeToolUse> {
	const assistant = decode(AssistantSchema, payload);
	if (assistant === undefined) return [];

	return assistant.message.content.flatMap((item) => {
		const tool = decode(ToolUseSchema, item);
		return tool === undefined ? [] : [tool];
	});
}

/**
 * Names the frames that exist purely for CLI bookkeeping. Every frame is
 * already retained verbatim as raw provenance, so re-emitting these as
 * canonical observations would only bury real diagnostics in noise.
 */
const silent_system_subtypes = new Set(["hook_started", "hook_response", "status"]);
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
		protocol_version: input.protocol_version ?? "claude-stream-json-v1",
		raw_frame_base64: input.raw_frame_base64,
		transport: input.transport ?? "claude-cli-stream-json",
	};

	return {
		artisan_run_id: input.artisan_run_id,
		native_thread_id: input.native_thread_id,
		observation_id: [input.artisan_run_id, "claude", String(input.frame_sequence), suffix]
			.filter((part) => part !== undefined)
			.join(":"),
		raw,
		sequence: 0,
	};
}

/**
 * Emits the one completed file change a successful tool result proves. The
 * frame's ordinary observation id remains source-unique: tool use ids are
 * provider lifecycle identities, not ledger admission ids.
 */
function applied_file_change(
	input: ClaudeNormalizationInput,
	applied: Schema.Schema.Type<typeof ToolUseResultSchema> | undefined,
	tool: ClaudeToolUse,
): EngineFileObservation | undefined {
	if (applied?.filePath === undefined) return undefined;
	if (applied.type !== "create" && applied.type !== "update") return undefined;

	const counts =
		applied.structuredPatch !== undefined && applied.structuredPatch.length > 0
			? CountPatchHunkLines(applied.structuredPatch)
			: applied.content === undefined
				? undefined
				: CountWrittenLines(applied.content, applied.originalFile ?? undefined);

	return {
		...make_base(input, "user.tool_result", tool.id),
		_tag: "file",
		action: applied.type === "create" ? "created" : "modified",
		...(counts === undefined ? {} : counts),
		path: applied.filePath,
	} satisfies EngineFileObservation;
}

/**
 * Completes a file tool even when Claude omitted its applied-edit metadata.
 *
 * The edit's own arguments are that patch. An edit names the block it replaced
 * and the block it wrote, so counting those reads the change the engine applied
 * from the request rather than inventing one — the same rule the applied-metadata
 * path follows, from the only other place the same fact is stated. Without a
 * replaced block there is nothing honest to count: a write discloses what it
 * wrote and never what stood there before, and half a count renders as a
 * confident zero.
 */
function correlated_file_change(
	input: ClaudeNormalizationInput,
	tool: ClaudeToolUse,
): EngineFileObservation {
	const input_value = tool_input(tool);
	const counts =
		input_value?.new_string === undefined
			? undefined
			: CountWrittenLines(input_value.new_string, input_value.old_string);

	return {
		...make_base(input, "user.tool_result", tool.id),
		_tag: "file",
		action: "modified",
		...(counts === undefined ? {} : counts),
		path: input_value?.file_path ?? input_value?.path ?? "unknown",
	} satisfies EngineFileObservation;
}

/** A failed file mutation has no completed file report, but must stay named. */
function failed_file_tool_observation(
	input: ClaudeNormalizationInput,
	tool: ClaudeToolUse,
): EngineToolObservation {
	const input_value = tool_input(tool);
	const detail = input_value?.file_path ?? input_value?.path;

	return {
		...make_base(input, "user.tool_result", tool.id),
		_tag: "tool",
		action: "failed",
		...(detail === undefined ? {} : { detail }),
		tool_id: tool.id,
		tool_name: tool.name,
	} satisfies EngineToolObservation;
}

/**
 * Identifies which file result one outer applied-edit report can enrich. A
 * Claude user frame may settle several parallel tools while exposing only one
 * `tool_use_result`; applying that singleton to every file result fabricates
 * duplicate paths and counts. A sole file result is unambiguous. With several,
 * the report is used only when its path names exactly one correlated request.
 */
function applied_file_tool_id(
	applied: Schema.Schema.Type<typeof ToolUseResultSchema> | undefined,
	results: ReadonlyArray<Schema.Schema.Type<typeof ToolResultSchema> | undefined>,
	tool_uses: ReadonlyMap<string, ClaudeToolUse> | undefined,
): string | undefined {
	if (applied?.filePath === undefined || tool_uses === undefined) return undefined;
	if (applied.type !== "create" && applied.type !== "update") return undefined;

	const file_tools = results.flatMap((result) => {
		if (result === undefined) return [];
		const tool = tool_uses.get(result.tool_use_id);
		return tool === undefined || !is_file_tool(tool) ? [] : [tool];
	});
	if (file_tools.length === 1) return file_tools[0]?.id;

	const path_matches = file_tools.filter((tool) => {
		const input_value = tool_input(tool);
		return (input_value?.file_path ?? input_value?.path) === applied.filePath;
	});
	return path_matches.length === 1 ? path_matches[0]?.id : undefined;
}

function native_action(
	input: ClaudeNormalizationInput,
	detail: string,
	error_ref?: EngineErrorRef,
) {
	return {
		...make_base(input, "unknown"),
		_tag: "native_action",
		action: "claude_event",
		detail,
		...(error_ref === undefined ? {} : { error_ref }),
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

function tool_input(tool: ClaudeToolUse) {
	return decode(
		Schema.Struct({
			args: Schema.optional(Schema.String),
			command: Schema.optional(Schema.String),
			description: Schema.optional(Schema.String),
			file_path: Schema.optional(Schema.String),
			new_string: Schema.optional(Schema.String),
			notebook_path: Schema.optional(Schema.String),
			old_string: Schema.optional(Schema.String),
			path: Schema.optional(Schema.String),
			pattern: Schema.optional(Schema.String),
			query: Schema.optional(Schema.String),
			url: Schema.optional(Schema.String),
		}),
		tool.input,
	);
}

/**
 * What a tool row says it did, taken from the one argument that identifies the
 * call: the file a read opened, the pattern a search matched, the task a
 * subagent was given.
 *
 * Every row of a kind otherwise renders as the same word, which is what a fixed
 * string saying the input lives in raw provenance amounted to — a sentence about
 * the adapter's bookkeeping standing where the work should be. When a call
 * genuinely discloses nothing identifying, this returns nothing and the row
 * keeps its normalized label instead of describing the absence.
 *
 * Ordered by how much each argument narrows the call down: a grep's pattern says
 * more than the directory it ran in, and a fetch's URL more than its prompt.
 */
function tool_detail(tool: ClaudeToolUse): string | undefined {
	const value = tool_input(tool);
	if (value === undefined) return undefined;

	const candidate =
		value.command ??
		value.file_path ??
		value.notebook_path ??
		value.pattern ??
		value.query ??
		value.url ??
		value.description ??
		value.path ??
		value.args;
	const detail = candidate?.trim();

	return detail === undefined || detail.length === 0 ? undefined : detail;
}

function is_file_tool(tool: ClaudeToolUse) {
	return ["Edit", "Write", "NotebookEdit"].includes(tool.name);
}

/**
 * Reading a file is file work, not anonymous tool work. Routed to the file
 * semantics so the row carries the path it opened and a chain of them counts
 * itself as files read rather than as tools used.
 */
function is_file_read_tool(tool: ClaudeToolUse) {
	return tool.name === "Read" || tool.name === "NotebookRead";
}

function tool_observation(
	input: ClaudeNormalizationInput,
	tool: ClaudeToolUse,
	action: EngineToolObservation["action"] = "started",
): EngineObservation | undefined {
	const input_value = tool_input(tool);
	const name = tool.name;

	/**
	 * Both shells, not just Bash: the Windows harness runs commands through a
	 * `PowerShell` tool, and matching only Bash sent every one of those to the
	 * generic tool shape — a chain of commands reading "Used 2 tools" with the
	 * command text riding along as an anonymous detail.
	 */
	if (name === "Bash" || name === "PowerShell") {
		return {
			...make_base(input, "assistant.tool_use", tool.id),
			_tag: "terminal_activity",
			activity_id: tool.id,
			...(input_value?.command === undefined ? {} : { command: input_value.command }),
			/**
			 * Which tool ran it is the only evidence of which interpreter the text
			 * was written for; nothing downstream can recover it from the command.
			 */
			shell: name === "PowerShell" ? "pwsh" : "bash",
			state:
				action === "failed" ? "failed" : action === "completed" ? "completed" : "started",
		} satisfies EngineTerminalActivityObservation;
	}

	if (is_file_tool(tool)) {
		/**
		 * A file observation is completed by definition in the public projection.
		 * Do not admit a count-less request here: it would persist as applied and
		 * block the later result frame that contains the actual patch counts.
		 */
		return undefined;
	}

	/**
	 * Same rule for a read: the projection keys a read row off the observation
	 * that reported it, so emitting one on the request and again on the result
	 * would leave two rows for one read. The result frame is the one that proves
	 * it happened. A failure keeps the ordinary tool shape, which is what carries
	 * a failed state at all.
	 */
	if (is_file_read_tool(tool)) {
		const path = input_value?.file_path ?? input_value?.notebook_path;
		if (path !== undefined && path.length > 0 && action !== "failed") {
			if (action === "started") return undefined;

			return {
				...make_base(input, "user.tool_result", tool.id),
				_tag: "file",
				action: "read",
				path,
			} satisfies EngineFileObservation;
		}
	}

	if (["WebSearch", "WebFetch"].includes(name)) {
		return {
			...make_base(input, "assistant.tool_use", tool.id),
			_tag: "search",
			query: input_value?.query ?? input_value?.url ?? "",
			scope: "web",
			search_id: tool.id,
			state: action === "started" ? "started" : "completed",
		} satisfies EngineSearchObservation;
	}

	/**
	 * Grep and Glob are searches of the workspace, not anonymous tool work.
	 * Routed to the search semantics with their scope so a chain of them
	 * counts itself as files searched rather than tools used, with each row
	 * carrying the pattern it matched.
	 */
	if (name === "Grep" || name === "Glob") {
		return {
			...make_base(input, "assistant.tool_use", tool.id),
			_tag: "search",
			/** The pattern names the search; a call without one at least names where it looked. */
			query: input_value?.pattern ?? input_value?.path ?? "",
			scope: "workspace",
			search_id: tool.id,
			state: action === "started" ? "started" : "completed",
		} satisfies EngineSearchObservation;
	}

	const detail = tool_detail(tool);

	return {
		...make_base(input, "assistant.tool_use", tool.id),
		_tag: "tool",
		action,
		...(detail === undefined ? {} : { detail }),
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
		const metadata = compact_boundary.compact_metadata ?? compact_boundary.compactMetadata;
		const duration_ms = compaction_duration_ms(metadata);
		const post_tokens = compaction_post_tokens(metadata);
		const observations: Array<EngineObservation> = [
			{
				...base,
				_tag: "compaction",
				...(compact_boundary.uuid === undefined
					? {}
					: { compaction_id: compact_boundary.uuid }),
				...(duration_ms === undefined || !Number.isFinite(duration_ms) || duration_ms < 0
					? {}
					: { duration_ms }),
				raw: {
					...base.raw,
					...(compact_boundary.uuid === undefined
						? {}
						: { native_id: compact_boundary.uuid }),
				},
				state: "completed",
			} satisfies EngineCompactionObservation,
		];
		/**
		 * A compaction rewrites what occupies the window, so the boundary's own
		 * post-compaction measurement resets the context gauge immediately
		 * instead of leaving the pre-compaction reading standing until the next
		 * assistant response happens to report.
		 */
		if (post_tokens !== undefined) {
			observations.push({
				...make_base(input, "system.compact_boundary", "compact_usage"),
				_tag: "usage",
				basis: "cumulative",
				context_tokens: post_tokens,
				turn_id: input.turn_id,
			} satisfies EngineUsageObservation);
		}
		return observations;
	}

	const task_lifecycle = normalize_claude_task_lifecycle(input);
	if (task_lifecycle !== undefined) return task_lifecycle;

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
	if (thinking_delta !== undefined) {
		/**
		 * An empty delta is not a summary of anything. Forwarding one opened a
		 * reasoning item with no text and nothing ever to put in it — 1,834 of
		 * them across this database, one per Claude thinking block, each
		 * rendering as a blank row that claimed the model had said something.
		 */
		if (thinking_delta.event.delta.thinking.length === 0) return [];
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
	}

	/**
	 * The thinking counter opens the same reasoning item the deltas would have,
	 * so a turn whose reasoning is encrypted still has one row that says the
	 * model is working and how hard, rather than nothing until it speaks.
	 */
	const thinking_tokens = decode(ThinkingTokensSchema, input.payload);
	if (thinking_tokens !== undefined) {
		if (thinking_tokens.estimated_tokens === undefined) return [];
		return [
			{
				...make_base(input, "system.thinking_tokens"),
				_tag: "reasoning_summary_delta",
				delta: "",
				item_id: `${message_item_id(input, undefined)}:reasoning`,
				summary_index: 0,
				thinking_tokens: thinking_tokens.estimated_tokens,
				turn_id: input.turn_id,
			} satisfies EngineReasoningSummaryDeltaObservation,
		];
	}

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
						/**
						 * Rejected and exceeded are terminal usage evidence. Warnings
						 * remain provenance the reader may notice, not error cards.
						 */
						rate_limit.rate_limit_info.status === "rejected" ||
							rate_limit.rate_limit_info.status === "exceeded"
							? classify_claude_rate_limit(
									rate_limit.rate_limit_info.resetsAt,
									rate_limit.rate_limit_info.rateLimitType,
								)
							: undefined,
					),
				];

	const assistant = decode(AssistantSchema, input.payload);
	if (assistant !== undefined) {
		const text_parts: Array<string> = [];
		const observations: Array<EngineObservation> = [];
		const has_tool_use = assistant.message.content.some(
			(item) => decode(ToolUseSchema, item) !== undefined,
		);
		const has_thinking = assistant.message.content.some(
			(item) => decode(ThinkingSchema, item) !== undefined,
		);
		assistant.message.content.forEach((item, index) => {
			const text = decode(TextSchema, item);
			const tool = decode(ToolUseSchema, item);
			if (text !== undefined) text_parts.push(text.text);
			if (tool !== undefined) {
				const observation = tool_observation(input, tool);
				if (observation !== undefined) observations.push(observation);
			}
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
		/**
		 * The typed per-message error must survive even when the frame also
		 * carries prose or usage — the human-readable failure text is a message,
		 * but the classification is what Artisan takes custody of, and the old
		 * early returns silently dropped it whenever anything else decoded.
		 */
		if (assistant.error !== undefined)
			observations.push(
				native_action(
					input,
					`Claude assistant error: ${assistant.error}`,
					classify_claude_assistant_error(assistant.error),
				),
			);
		if (observations.length > 0) return observations;
		if (has_tool_use) return [];
		return [native_action(input, "Assistant event without public content")];
	}

	const user = decode(UserSchema, input.payload);
	if (user !== undefined) {
		const applied = decode(ToolUseResultSchema, user.tool_use_result);
		const results = user.message.content.map((item) => decode(ToolResultSchema, item));
		const applied_tool_id = applied_file_tool_id(applied, results, input.tool_uses);

		return results.map((result, index) => {
			if (result === undefined)
				return native_action(input, `Malformed user content at index ${index}`);

			const tool = input.tool_uses?.get(result.tool_use_id);
			if (tool !== undefined) {
				if (is_file_tool(tool)) {
					if (result.is_error === true) return failed_file_tool_observation(input, tool);

					const file_change =
						result.tool_use_id === applied_tool_id
							? applied_file_change(input, applied, tool)
							: undefined;
					return file_change ?? correlated_file_change(input, tool);
				}

				const observation = tool_observation(
					input,
					tool,
					result.is_error === true ? "failed" : "completed",
				);
				if (observation !== undefined) return observation;
			}

			/**
			 * A result whose request never reached this adapter. Nothing about the
			 * call is known beyond that it ended, so the row carries no detail and
			 * keeps its normalized label — a fixed sentence about where the content
			 * was kept described the adapter, not the run.
			 */
			return {
				...make_base(input, "user.tool_result", result.tool_use_id),
				_tag: "tool",
				action: result.is_error === true ? "failed" : "completed",
				tool_id: result.tool_use_id,
				tool_name: "claude-tool",
			} satisfies EngineToolObservation;
		});
	}

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
		/**
		 * A failed result frame usually also carries usage; the failure must not
		 * lose to it. The classified action is what settles the run's error card,
		 * so it rides alongside whatever else the frame reported.
		 */
		if (result.subtype !== "success" || result.is_error === true)
			observations.push(
				native_action(
					input,
					`Claude result failure: ${result.subtype}`,
					classify_claude_result_failure(result.subtype),
				),
			);
		if (observations.length > 0) return observations;
		return [native_action(input, "Claude result")];
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
					classify_claude_terminal_failure(terminal.stop_reason ?? undefined),
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
