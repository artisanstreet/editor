import { Data, Effect, Schema, Scope, Stream } from "effect";

/** Finite token quantity emitted by a provider. */
export const TokenCount = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0));
export type TokenCount = typeof TokenCount.Type;

/** Describes the maturity of one provider capability. @since 0.2.0 */
export type EngineCapabilityState = "supported" | "experimental" | "unsupported";

/** Names a capability declared by an engine adapter. @since 0.2.0 */
export type EngineCapabilityName =
	| "approval"
	| "auth"
	| "cancel"
	| "close"
	| "events"
	| "global_guidance"
	| "model_selection"
	| "native_continuation"
	| "native_tools"
	| "probe"
	| "question"
	| "raw_frames"
	| "resume"
	| "start"
	| "steer"
	| "subagents";

/** States the availability and optional limitation of one engine capability. @since 0.2.0 */
export interface EngineCapability {
	readonly reason?: string;
	readonly state: EngineCapabilityState;
}

/** Lists every capability declared by an engine adapter. @since 0.2.0 */
export type EngineCapabilities = Readonly<Record<EngineCapabilityName, EngineCapability>>;

/** Identifies an installed engine adapter and the contract it currently supports. @since 0.2.0 */
export interface EngineDescriptor {
	readonly capabilities: EngineCapabilities;
	readonly display_name: string;
	readonly id: string;
	readonly transport: string;
}

/** Defines provider-neutral approval and runtime access requested for one run. @since 0.3.0 */
export interface EnginePermissionPolicy {
	readonly approval: "never" | "on_request" | "always";
	/**
	 * Widens writable filesystem access beyond the workspace when explicitly
	 * set. Omitted writable policies retain the historical workspace boundary.
	 */
	readonly edit_scope?: "workspace" | "host";
	readonly network_access: boolean;
	readonly write_access: boolean;
}

/** Represents one provider-scoped adapter option value. @since 0.3.0 */
export type EngineProviderOptionValue = string | boolean | number | null;

/** Carries canonical policy and provider-owned preferences chosen by the caller. @since 0.2.0 */
export interface EngineRunMetadata {
	readonly model?: string;
	readonly permission_policy?: EnginePermissionPolicy;
	readonly provider_options?: Readonly<Record<string, EngineProviderOptionValue>>;
}

/**
 * Carries normalized global guidance and the canonical file from which it was loaded.
 *
 * @example
 * ```ts
 * const guidance: EngineGlobalGuidance = {
 *   content: "Keep user instructions separate from project guidance.",
 *   source_file: "C:\\workspace\\AGENTS.md",
 * };
 * ```
 *
 * @since 0.5.0
 */
export interface EngineGlobalGuidance {
	readonly content: string;
	readonly source_file: string;
}

/** Carries product-owned instructions separately from global guidance and user content. */
export interface EngineProductInstructions {
	readonly content: string;
	readonly source: string;
}

/** Supplies the caller-owned context shared by started and resumed runs. @since 0.2.0 */
export interface EngineRunContext extends EngineRunMetadata {
	readonly artisan_run_id: string;
	readonly global_guidance?: EngineGlobalGuidance;
	readonly product_instructions?: EngineProductInstructions;
	readonly working_directory: string;
}

/**
 * Validates global guidance before an adapter contacts its provider.
 *
 * @example
 * ```ts
 * yield* ValidateEngineGlobalGuidance("codex", input.global_guidance);
 * ```
 *
 * @since 0.5.0
 * @param engine_id - Stable adapter identifier included in a configuration failure.
 * @param global_guidance - Optional normalized guidance selected for this run.
 * @returns An effect that completes when the source file is nonempty.
 */
export function ValidateEngineGlobalGuidance(
	engine_id: string,
	global_guidance: EngineGlobalGuidance | undefined,
): Effect.Effect<void, EngineConfigurationError> {
	return global_guidance === undefined ||
		(typeof global_guidance.source_file === "string" &&
			global_guidance.source_file.trim().length > 0)
		? Effect.void
		: Effect.fail(
				new EngineConfigurationError({
					engine_id,
					option: "global_guidance.source_file",
					value: global_guidance.source_file,
				}),
			);
}

/** Validates product-owned instructions before an adapter contacts its provider. */
export const ValidateEngineProductInstructions = (
	engine_id: string,
	product_instructions: EngineProductInstructions | undefined,
): Effect.Effect<void, EngineConfigurationError> => {
	if (product_instructions === undefined) return Effect.void;

	if (
		typeof product_instructions.content !== "string" ||
		product_instructions.content.trim().length === 0
	) {
		return Effect.fail(
			new EngineConfigurationError({
				engine_id,
				option: "product_instructions.content",
				value: product_instructions.content,
			}),
		);
	}

	return typeof product_instructions.source !== "string" ||
		product_instructions.source.trim().length === 0
		? Effect.fail(
				new EngineConfigurationError({
					engine_id,
					option: "product_instructions.source",
					value: product_instructions.source,
				}),
			)
		: Effect.void;
};

/** Carries provider-owned state that can reopen a run without inventing a checkpoint. @since 0.2.0 */
export interface EngineResumeToken {
	readonly native_thread_id: string;
	readonly opaque_checkpoint?: string;
}

/** Provider-neutral, ordered multimodal input supplied with a user turn. */
export type EngineUserInputPart =
	| { readonly text: string; readonly type: "text" }
	| {
			readonly bytes: Uint8Array;
			readonly id: string;
			readonly media_type: "image/gif" | "image/jpeg" | "image/png" | "image/webp";
			readonly name: string;
			readonly type: "image";
	  };

/** Opens a new native thread from initial user text. @since 0.2.0 */
export interface EngineStartInput extends EngineRunContext {
	readonly _tag: "start";
	/** When supplied, retains the user's exact text/image ordering. */
	readonly initial_content?: ReadonlyArray<EngineUserInputPart>;
	readonly initial_text: string;
}

/** Reopens provider-owned state and may continue it with new user text. @since 0.2.0 */
export interface EngineResumeInput extends EngineRunContext {
	readonly _tag: "resume";
	/** When supplied, retains the next user's exact text/image ordering. */
	readonly next_content?: ReadonlyArray<EngineUserInputPart>;
	readonly next_text?: string;
	readonly resume_token: EngineResumeToken;
}

/** Selects whether a run starts fresh or resumes provider-owned state. @since 0.2.0 */
export type EngineOpenInput = EngineResumeInput | EngineStartInput;

/** Asks an adapter whether one of its own native sessions can change model safely. */
export interface EngineNativeContinuationInput {
	readonly resume_token: EngineResumeToken;
	readonly source_model?: string;
	readonly target_model?: string;
}

/** Reports an explicit, version-tested native continuation decision. */
export type EngineNativeContinuationDecision =
	| { readonly state: "compatible" }
	| {
			readonly reason: string;
			readonly state: "incompatible" | "unsupported";
	  };

/** Supplies caller metadata for a non-billable availability probe. @since 0.2.0 */
export interface EngineProbeInput {
	readonly client_name?: string;
	readonly client_version?: string;
}

/** Reports whether a provider can authenticate a caller without starting a run. @since 0.2.0 */
export interface EngineAuthReadiness {
	readonly reason?: string;
	readonly state: "authenticated" | "unauthenticated" | "unknown";
}

/** Reports non-billable version, authentication, and capability readiness. @since 0.2.0 */
export interface EngineProbe {
	readonly authentication: EngineAuthReadiness;
	readonly capabilities: EngineCapabilities;
	readonly descriptor: EngineDescriptor;
	readonly metadata: Readonly<Record<string, string>>;
	readonly ready: boolean;
	readonly version: string;
}

/** Captures immutable origin and raw-frame provenance for one canonical observation. @since 0.2.0 */
export interface EngineRawProvenance {
	readonly engine_id: string;
	readonly frame: unknown;
	readonly frame_sequence?: number;
	readonly native_id?: string | number;
	readonly native_method?: string;
	readonly protocol_version?: string;
	readonly raw_frame_base64?: string;
	readonly transport: string;
}

/** Supplies the provider-neutral fields present on every observation. @since 0.2.0 */
export interface EngineObservationBase {
	readonly artisan_run_id: string;
	/** Provider thread that emitted this observation when the native protocol exposes one. */
	readonly native_thread_id?: string;
	readonly observation_id: string;
	readonly raw: EngineRawProvenance;
	readonly sequence: number;
}

/** Reports a non-terminal lifecycle change for the run. @since 0.2.0 */
export interface EngineRunStateObservation extends EngineObservationBase {
	readonly _tag: "run_state";
	readonly state: "opening" | "running" | "waiting";
}

/** Reports lifecycle progress for a single provider turn. @since 0.2.0 */
export interface EngineTurnStateObservation extends EngineObservationBase {
	readonly _tag: "turn_state";
	readonly state: "started" | "waiting" | "completed" | "cancelled" | "failed";
	readonly turn_id: string;
}

/** Reports observable provider-native subagent activity without making it the owner run. @since 0.10.0 */
export interface EngineSubagentObservation extends EngineObservationBase {
	readonly _tag: "subagent";
	readonly agent_native_thread_id: string;
	readonly parent_native_thread_id: string;
	readonly activity?: string;
	readonly agent_path?: string;
	readonly state: "discovered" | "running" | "waiting" | "completed" | "failed" | "interrupted";
	readonly turn_id?: string;
}

/** Public, provider-neutral content emitted by one native subagent. */
const AssistantMessagePhaseSchema = Schema.Union([
	Schema.Literal("commentary"),
	Schema.Literal("final"),
	Schema.Literal("unspecified"),
]);
const TerminalChannelSchema = Schema.Union([Schema.Literal("stdout"), Schema.Literal("stderr")]);
const TerminalActivityStateSchema = Schema.Union([
	Schema.Literal("started"),
	Schema.Literal("output"),
	Schema.Literal("completed"),
	Schema.Literal("failed"),
]);
const ToolActionSchema = Schema.Union([
	Schema.Literal("started"),
	Schema.Literal("progress"),
	Schema.Literal("completed"),
	Schema.Literal("failed"),
]);
const FileActionSchema = Schema.Union([
	Schema.Literal("created"),
	Schema.Literal("modified"),
	Schema.Literal("deleted"),
	Schema.Literal("read"),
]);
const SearchScopeSchema = Schema.Union([Schema.Literal("workspace"), Schema.Literal("web")]);
const SearchStateSchema = Schema.Union([Schema.Literal("started"), Schema.Literal("completed")]);
export const EngineSubagentTranscriptContent = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("agent_message_delta"),
		delta: Schema.String,
		item_id: Schema.String,
		phase: AssistantMessagePhaseSchema,
	}),
	Schema.Struct({
		_tag: Schema.Literal("agent_message_completed"),
		item_id: Schema.String,
		message: Schema.String,
		phase: AssistantMessagePhaseSchema,
	}),
	Schema.Struct({
		_tag: Schema.Literal("reasoning_summary_delta"),
		delta: Schema.String,
		item_id: Schema.String,
		summary_index: Schema.Int,
	}),
	Schema.Struct({
		_tag: Schema.Literal("reasoning_summary_completed"),
		item_id: Schema.String,
		text: Schema.optional(Schema.String),
	}),
	Schema.Struct({
		_tag: Schema.Literal("terminal_activity"),
		activity_id: Schema.String,
		channel: Schema.optional(TerminalChannelSchema),
		command: Schema.optional(Schema.String),
		exit_code: Schema.optional(Schema.Int),
		output: Schema.optional(Schema.String),
		state: TerminalActivityStateSchema,
	}),
	Schema.Struct({
		_tag: Schema.Literal("tool"),
		action: ToolActionSchema,
		detail: Schema.optional(Schema.String),
		tool_id: Schema.String,
		tool_name: Schema.String,
	}),
	Schema.Struct({
		_tag: Schema.Literal("file"),
		action: FileActionSchema,
		lines_added: Schema.optional(Schema.Int),
		lines_deleted: Schema.optional(Schema.Int),
		path: Schema.String,
	}),
	Schema.Struct({
		_tag: Schema.Literal("search"),
		query: Schema.String,
		result_count: Schema.optional(Schema.Int),
		scope: Schema.optional(SearchScopeSchema),
		search_id: Schema.optional(Schema.String),
		state: SearchStateSchema,
	}),
]);
export type EngineSubagentTranscriptContent = typeof EngineSubagentTranscriptContent.Type;

/** Keeps child transcript provenance separate from the root conversation. */
export interface EngineSubagentTranscriptObservation extends EngineObservationBase {
	readonly _tag: "subagent_transcript";
	readonly agent_native_thread_id: string;
	readonly content: EngineSubagentTranscriptContent;
	readonly parent_native_thread_id: string;
}

/** Converts renderer-safe ordinary content into an isolated native-child event. */
export function MakeEngineSubagentTranscriptObservation(input: {
	readonly agent_native_thread_id: string;
	readonly observation: EngineObservation;
	readonly parent_native_thread_id: string;
}): EngineSubagentTranscriptObservation | undefined {
	const { observation } = input;
	const content: EngineSubagentTranscriptContent | undefined = (() => {
		switch (observation._tag) {
			case "agent_message_delta":
				return {
					_tag: observation._tag,
					delta: observation.delta,
					item_id: observation.item_id,
					phase: observation.phase,
				};
			case "agent_message_completed":
				return {
					_tag: observation._tag,
					item_id: observation.item_id,
					message: observation.message,
					phase: observation.phase,
				};
			case "reasoning_summary_delta":
				return {
					_tag: observation._tag,
					delta: observation.delta,
					item_id: observation.item_id,
					summary_index: observation.summary_index,
				};
			case "reasoning_summary_completed":
				return {
					_tag: observation._tag,
					item_id: observation.item_id,
					...(observation.text === undefined ? {} : { text: observation.text }),
				};
			case "terminal_activity":
				return {
					_tag: observation._tag,
					activity_id: observation.activity_id,
					...(observation.channel === undefined ? {} : { channel: observation.channel }),
					...(observation.command === undefined ? {} : { command: observation.command }),
					...(observation.exit_code === undefined
						? {}
						: { exit_code: observation.exit_code }),
					...(observation.output === undefined ? {} : { output: observation.output }),
					state: observation.state,
				};
			case "tool":
				return {
					_tag: observation._tag,
					action: observation.action,
					...(observation.detail === undefined ? {} : { detail: observation.detail }),
					tool_id: observation.tool_id,
					tool_name: observation.tool_name,
				};
			case "file":
				return {
					_tag: observation._tag,
					action: observation.action,
					...(observation.lines_added === undefined
						? {}
						: { lines_added: observation.lines_added }),
					...(observation.lines_deleted === undefined
						? {}
						: { lines_deleted: observation.lines_deleted }),
					path: observation.path,
				};
			case "search":
				return {
					_tag: observation._tag,
					query: observation.query,
					...(observation.result_count === undefined
						? {}
						: { result_count: observation.result_count }),
					...(observation.scope === undefined ? {} : { scope: observation.scope }),
					...(observation.search_id === undefined
						? {}
						: { search_id: observation.search_id }),
					state: observation.state,
				};
			default:
				return undefined;
		}
	})();
	if (content === undefined) return undefined;
	return {
		_tag: "subagent_transcript",
		agent_native_thread_id: input.agent_native_thread_id,
		artisan_run_id: observation.artisan_run_id,
		content,
		observation_id: `${observation.observation_id}:subagent-transcript`,
		parent_native_thread_id: input.parent_native_thread_id,
		raw: observation.raw,
		sequence: observation.raw.frame_sequence ?? observation.sequence,
	};
}

/** Streams a partial agent-authored message. @since 0.2.0 */
export interface EngineAgentMessageDeltaObservation extends EngineObservationBase {
	readonly _tag: "agent_message_delta";
	readonly delta: string;
	/** Identifies the native assistant message item this delta extends. */
	readonly item_id: string;
	/** Preserves a provider-disclosed display phase without inferring it from message order. */
	readonly phase: EngineAssistantMessagePhase;
	readonly turn_id: string;
}

/** Distinguishes progress commentary from a settled assistant reply. @since 0.4.0 */
export type EngineAssistantMessagePhase = "commentary" | "final" | "unspecified";

/** Completes one agent-authored message. @since 0.2.0 */
export interface EngineAgentMessageCompletedObservation extends EngineObservationBase {
	readonly _tag: "agent_message_completed";
	/** Identifies the native assistant message item completed by this observation. */
	readonly item_id: string;
	readonly message: string;
	/** Preserves a provider-disclosed display phase without inferring it from message order. */
	readonly phase: EngineAssistantMessagePhase;
	readonly turn_id: string;
}

/** Describes a provider-neutral plan update. @since 0.2.0 */
export interface EnginePlanObservation extends EngineObservationBase {
	readonly _tag: "plan";
	readonly entries: ReadonlyArray<{
		readonly id: string;
		readonly status: "pending" | "in_progress" | "completed";
		readonly text: string;
	}>;
	readonly turn_id?: string;
}

/** Reports the sole terminal outcome emitted by a run. @since 0.2.0 */
export interface EngineRunTerminalObservation extends EngineObservationBase {
	readonly _tag: "run_terminal";
	/**
	 * The renderer-safe explanation for a failed terminal result. This is
	 * Artisan-owned, catalog-backed evidence; adapters keep their raw provider
	 * failure in `raw` rather than placing it here.
	 */
	readonly error_ref?: EngineErrorRef;
	readonly state: EngineRunTerminalState;
}

/** Names the only outcomes that can complete an engine run. @since 0.2.0 */
export type EngineRunTerminalState = "completed" | "cancelled" | "failed" | "closed";

/** Reports shell or process activity independently from the run outcome. @since 0.2.0 */
export interface EngineTerminalActivityObservation extends EngineObservationBase {
	readonly _tag: "terminal_activity";
	readonly activity_id: string;
	readonly channel?: "stdout" | "stderr";
	readonly command?: string;
	readonly exit_code?: number;
	readonly output?: string;
	readonly state: "started" | "output" | "completed" | "failed";
}

/** Reports a tool lifecycle event without exposing provider-specific tool types. @since 0.2.0 */
export interface EngineToolObservation extends EngineObservationBase {
	readonly _tag: "tool";
	readonly action: "started" | "progress" | "completed" | "failed";
	readonly detail?: string;
	readonly tool_id: string;
	readonly tool_name: string;
}

/** Reports a file mutation or inspection performed during a run. @since 0.2.0 */
export interface EngineFileObservation extends EngineObservationBase {
	readonly _tag: "file";
	readonly action: "created" | "modified" | "deleted" | "read";
	/**
	 * The lines this run's own patch added and removed, when the engine reported
	 * enough to count them. Counted from the applied patch rather than the working
	 * tree: a diff of the tree cannot say which run made which change, and it
	 * needs a repository that a workspace may not have. Absent means uncounted,
	 * never zero — a renderer must distinguish "no change" from "not reported".
	 * @since 0.3.0
	 */
	readonly lines_added?: number;
	readonly lines_deleted?: number;
	readonly path: string;
}

/** Reports a search operation performed during a run. @since 0.2.0 */
export interface EngineSearchObservation extends EngineObservationBase {
	readonly _tag: "search";
	readonly query: string;
	readonly result_count?: number;
	/**
	 * Where the search looked. A workspace search is file work and counts
	 * itself as "searched files"; a web search reads as "searched the web".
	 * Absent means web — the only scope this observation reported before the
	 * field existed, kept as the default so existing emitters stay honest.
	 * @since 0.8.0
	 */
	readonly scope?: "workspace" | "web";
	/**
	 * The provider's own id for this search, when it has one. A search is
	 * reported at least twice — once starting, once finished — and without an id
	 * of its own each report is only identifiable by the frame that carried it,
	 * which makes one search look like several and leaves the first of them
	 * running forever.
	 * @since 0.3.0
	 */
	readonly search_id?: string;
	readonly state: "started" | "completed";
}

/**
 * Transfers a provider failure into Artisan's custody: the adapter translates
 * the provider's typed signal into a stable `AE-*` code at the boundary, so
 * everything downstream reasons in Artisan's error vocabulary while the
 * provider's own code and message ride along as evidence. @since 0.9.0
 */
export interface EngineErrorRef {
	readonly artisan_code: string;
	/** The exact provider model whose independent allowance was depleted, when disclosed. */
	readonly affected_model_id?: string;
	readonly detail?: string;
	/** The provider's stable quota-bucket identifier, when disclosed. */
	readonly limit_id?: string;
	/** The provider's human-readable quota-bucket label, when disclosed. */
	readonly limit_label?: string;
	/** Whether the depleted allowance is shared, model-specific, or not disclosed. */
	readonly limit_scope?: "shared" | "model" | "unknown";
	readonly provider_code?: string;
	/** When a limit-class failure clears, as an ISO timestamp the UI can show. */
	readonly resets_at?: string;
}

/** Reports a provider-native action that has no canonical tool equivalent. @since 0.2.0 */
export interface EngineNativeActionObservation extends EngineObservationBase {
	readonly _tag: "native_action";
	readonly action: string;
	readonly detail?: string;
	/**
	 * Marks a frame the adapter could not interpret rather than something the
	 * provider did. Providers add frames faster than adapters model them, so
	 * this is ordinary drift, not a fault — the frame is already kept verbatim
	 * in raw provenance, and a reader has no use for a second copy of it in the
	 * transcript. @since 0.7.0
	 */
	readonly diagnostic?: boolean;
	/** Present when the action reports a failure the adapter classified. @since 0.9.0 */
	readonly error_ref?: EngineErrorRef;
}

/** Describes the provider-neutral action bound to an approval. @since 0.3.0 */
export type EngineApprovalRequest =
	| {
			readonly command?: string;
			readonly cwd?: string;
			readonly kind: "command";
			readonly reason?: string;
	  }
	| {
			readonly kind: "file_change";
			readonly reason?: string;
	  }
	| {
			readonly kind: "action";
			readonly reason?: string;
	  };

/** Reports an approval request or its resolution. @since 0.2.0 */
export interface EngineApprovalObservation extends EngineObservationBase {
	readonly _tag: "approval";
	readonly approval_id: string;
	readonly approved?: boolean;
	readonly description: string;
	readonly request: EngineApprovalRequest;
	readonly state: "requested" | "resolved";
}

/** Reports a question request or its resolution. @since 0.2.0 */
export interface EngineQuestionObservation extends EngineObservationBase {
	readonly _tag: "question";
	readonly answers?: ReadonlyArray<string>;
	readonly question_id: string;
	readonly state: "requested" | "resolved";
	readonly text: string;
}

/** Reports provider context compaction. @since 0.2.0 */
export interface EngineCompactionObservation extends EngineObservationBase {
	readonly _tag: "compaction";
	/**
	 * Provider-native lifecycle identity when the transport exposes one. This
	 * lets a started compaction resolve onto its later completion; providers
	 * that only announce completion deliberately leave it absent.
	 */
	readonly compaction_id?: string;
	readonly state: "started" | "completed";
	readonly summary?: string;
}

/** Streams a provider-authored reasoning summary without exposing private reasoning text. @since 0.3.0 */
export interface EngineReasoningSummaryDeltaObservation extends EngineObservationBase {
	readonly _tag: "reasoning_summary_delta";
	readonly delta: string;
	readonly item_id: string;
	readonly summary_index: number;
	readonly turn_id: string;
}

/**
 * Closes a reasoning phase for one turn. Providers whose reasoning display is
 * suppressed (for example Claude models with omitted thinking display) may
 * complete a phase that streamed no delta at all, so consumers must settle
 * reasoning state on this observation rather than on delta arrival. When a
 * provider supplies `text`, it is the authoritative public summary and may
 * replace previously streamed public deltas.
 *
 * @since 0.6.0
 */
export interface EngineReasoningSummaryCompletedObservation extends EngineObservationBase {
	readonly _tag: "reasoning_summary_completed";
	readonly item_id: string;
	readonly text?: string | undefined;
	readonly turn_id: string;
}

/** Reports provider usage measured for the run or one turn. @since 0.2.0 */
export interface EngineUsageObservation extends EngineObservationBase {
	readonly _tag: "usage";
	/** States whether counts add to prior observations or replace a provider-reported total. */
	readonly basis: "delta" | "cumulative" | "unknown";
	readonly cached_input_tokens?: number;
	/**
	 * Tokens occupying the model's context window when the provider measured
	 * this observation. A point-in-time gauge, never additive: a newer report
	 * replaces an older one regardless of `basis`.
	 */
	readonly context_tokens?: number;
	/**
	 * The provider-reported usable context window in tokens, when the wire
	 * discloses one. Providers may already subtract reserved headroom, so this
	 * is the denominator for `context_tokens`, not the raw model limit.
	 */
	readonly context_window_tokens?: number;
	readonly input_tokens?: number;
	readonly output_tokens?: number;
	readonly turn_id?: string;
}

/**
 * Reports a provider error and whether the provider will continue the current attempt.
 *
 * `attempt_state` intentionally models only the lifecycle disclosed by the provider;
 * adapters must not synthesize an attempt number when native data does not include one.
 */
export interface EngineRetryObservation extends EngineObservationBase {
	readonly _tag: "retry";
	readonly attempt_state: "retrying" | "terminal";
	readonly message: string;
	readonly turn_id: string;
	readonly will_retry: boolean;
}

/** Reports a decoded transport or protocol diagnostic. @since 0.2.0 */
export interface EngineProtocolDiagnosticObservation extends EngineObservationBase {
	readonly _tag: "protocol_diagnostic";
	readonly level: "info" | "warning" | "error";
	readonly message: string;
}

/** Reports a process-level diagnostic from the engine host. @since 0.2.0 */
export interface EngineProcessDiagnosticObservation extends EngineObservationBase {
	readonly _tag: "process_diagnostic";
	readonly level: "info" | "warning" | "error";
	readonly message: string;
	/** Present when the diagnostic reports a failure the host classified. @since 0.9.0 */
	readonly error_ref?: EngineErrorRef;
}

/** Defines the ordered, provider-neutral event stream emitted by a run. @since 0.2.0 */
export type EngineObservation =
	| EngineAgentMessageCompletedObservation
	| EngineAgentMessageDeltaObservation
	| EngineApprovalObservation
	| EngineCompactionObservation
	| EngineFileObservation
	| EngineNativeActionObservation
	| EnginePlanObservation
	| EngineProcessDiagnosticObservation
	| EngineProtocolDiagnosticObservation
	| EngineQuestionObservation
	| EngineReasoningSummaryCompletedObservation
	| EngineReasoningSummaryDeltaObservation
	| EngineRetryObservation
	| EngineRunStateObservation
	| EngineRunTerminalObservation
	| EngineSearchObservation
	| EngineSubagentObservation
	| EngineSubagentTranscriptObservation
	| EngineTerminalActivityObservation
	| EngineToolObservation
	| EngineTurnStateObservation
	| EngineUsageObservation;

/** Steers an active run with new user text. @since 0.2.0 */
export interface EngineSteerCommand {
	readonly _tag: "steer";
	readonly command_id: string;
	/** When supplied, retains the user's exact text/image ordering. */
	readonly content?: ReadonlyArray<EngineUserInputPart> | undefined;
	readonly text: string;
}

/** Answers a pending approval request. @since 0.2.0 */
export interface EngineRespondApprovalCommand {
	readonly _tag: "respond_approval";
	readonly approval_id: string;
	readonly approved: boolean;
	readonly command_id: string;
}

/** Answers a pending question. @since 0.2.0 */
export interface EngineRespondQuestionCommand {
	readonly _tag: "respond_question";
	readonly answers: Readonly<Record<string, ReadonlyArray<string>>>;
	readonly command_id: string;
}

/** Cancels the active run. @since 0.2.0 */
export interface EngineCancelCommand {
	readonly _tag: "cancel";
	readonly command_id: string;
}

/** Closes the run and releases its scoped resources. @since 0.2.0 */
export interface EngineCloseCommand {
	readonly _tag: "close";
	readonly command_id: string;
}

/** Defines every command accepted by a live engine run. @since 0.2.0 */
export type EngineCommand =
	| EngineCancelCommand
	| EngineCloseCommand
	| EngineRespondApprovalCommand
	| EngineRespondQuestionCommand
	| EngineSteerCommand;

/** Represents an unavailable executable or provider connection. @since 0.2.0 */
export class EngineUnavailableError extends Data.TaggedError("EngineUnavailableError")<{
	readonly engine_id: string;
	readonly message: string;
	/**
	 * The Artisan error code for why the engine is unavailable, when the
	 * adapter can classify it — an auth-gated probe knows it failed on
	 * sign-in, not on some generic startup fault. Consumers that settle the
	 * failure into the transcript prefer this over their own guess. @since 0.9.0
	 */
	readonly artisan_code?: string;
}> {}

/** Represents a capability deliberately absent from an adapter. @since 0.2.0 */
export class EngineUnsupportedOperationError extends Data.TaggedError(
	"EngineUnsupportedOperationError",
)<{
	readonly engine_id: string;
	readonly operation: EngineCapabilityName | "open";
}> {}

/** Represents a command unsupported by the live run. @since 0.2.0 */
export class EngineUnsupportedCommandError extends Data.TaggedError(
	"EngineUnsupportedCommandError",
)<{
	readonly command_id: string;
	readonly engine_id: string;
	readonly command: EngineCommand["_tag"];
}> {}

/** Represents a command rejected because the run already reached a terminal state. @since 0.2.0 */
export class EngineRunClosedError extends Data.TaggedError("EngineRunClosedError")<{
	readonly artisan_run_id: string;
	readonly command_id: string;
}> {}

/** Represents reuse of an accepted command identifier with changed intent. @since 0.2.0 */
export class EngineCommandIdConflictError extends Data.TaggedError("EngineCommandIdConflictError")<{
	readonly artisan_run_id: string;
	readonly command_id: string;
}> {}

/** Represents a command whose provider request target is absent or already resolved. @since 0.3.0 */
export class EngineCommandTargetError extends Data.TaggedError("EngineCommandTargetError")<{
	readonly artisan_run_id: string;
	readonly command_id: string;
	readonly target_id: string;
	readonly target: "approval" | "question";
}> {}

/** Represents a command rejected because the local event buffer is full. @since 0.2.0 */
export class EngineBackpressureError extends Data.TaggedError("EngineBackpressureError")<{
	readonly artisan_run_id: string;
	readonly capacity: number;
}> {}

/** Represents invalid adapter configuration rejected before provider work begins. @since 0.3.0 */
export class EngineConfigurationError extends Data.TaggedError("EngineConfigurationError")<{
	readonly engine_id: string;
	readonly option: string;
	readonly value: unknown;
}> {}

/** Represents a non-billable probe phase exceeding its configured deadline. @since 0.2.0 */
export class EngineProbeTimeoutError extends Data.TaggedError("EngineProbeTimeoutError")<{
	readonly engine_id: string;
	readonly phase: "initialize" | "version" | "authentication";
	readonly timeout_ms: number;
}> {}

/** Represents an invalid or incompatible provider transport message. @since 0.2.0 */
export class EngineProtocolError extends Data.TaggedError("EngineProtocolError")<{
	readonly engine_id: string;
	readonly message: string;
}> {}

/** Represents a process boundary failure while operating an engine. @since 0.2.0 */
export class EngineProcessError extends Data.TaggedError("EngineProcessError")<{
	readonly cause: unknown;
	readonly operation: "close" | "exit" | "kill" | "read" | "spawn" | "write";
}> {}

/** Represents a duplicate or unknown engine registration. @since 0.2.0 */
export class EngineRegistryError extends Data.TaggedError("EngineRegistryError")<{
	readonly engine_id: string;
	readonly reason: "duplicate_id" | "not_found";
}> {}

/** Enumerates failures emitted while opening or probing an engine. @since 0.2.0 */
export type EngineFailure =
	| EngineBackpressureError
	| EngineCommandIdConflictError
	| EngineCommandTargetError
	| EngineConfigurationError
	| EngineProbeTimeoutError
	| EngineProcessError
	| EngineProtocolError
	| EngineRegistryError
	| EngineRunClosedError
	| EngineUnavailableError
	| EngineUnsupportedCommandError
	| EngineUnsupportedOperationError;

/** Enumerates failures emitted by run command delivery. @since 0.2.0 */
export type EngineCommandFailure =
	| EngineBackpressureError
	| EngineCommandIdConflictError
	| EngineCommandTargetError
	| EngineProcessError
	| EngineProtocolError
	| EngineRunClosedError
	| EngineUnsupportedCommandError;

/** Owns one live engine run until its parent scope finalizes. @since 0.2.0 */
export interface EngineRun {
	readonly artisan_run_id: string;
	readonly Closed: Effect.Effect<EngineRunTerminalState>;
	/**
	 * Emits ordered, non-replay observations to exactly one consumer. A second
	 * consumer competes for values rather than receiving a copy. Sequence numbers
	 * are monotonic within this live stream but are not durable storage.
	 */
	readonly Events: Stream.Stream<EngineObservation>;
	readonly native_thread_id: string;
	readonly resume_token: EngineResumeToken;
	/**
	 * Delivers one command with intent-based idempotency. Retrying an accepted
	 * command identifier with identical intent succeeds without another side
	 * effect. Reusing it with changed intent fails with
	 * `EngineCommandIdConflictError`.
	 */
	readonly Send: (command: EngineCommand) => Effect.Effect<void, EngineCommandFailure>;
}

/** Classifies one provider quota window by its billing cadence. @since 0.6.0 */
export type EngineQuotaWindowKind = "session" | "weekly" | "monthly" | "unknown";

/**
 * Reports one provider-account quota window as a used percentage.
 *
 * `id` is the provider's stable bucket identifier (for example `five_hour` or
 * `codex_bengalfox`); `label` carries the provider's human bucket name when one
 * exists (for example a model-scoped weekly limit). `resets_at` is an ISO 8601
 * UTC timestamp.
 *
 * @since 0.6.0
 */
export interface EngineQuotaWindow {
	readonly id: string;
	readonly kind: EngineQuotaWindowKind;
	readonly label?: string;
	readonly percent_used: number;
	readonly resets_at?: string;
	/** Provider-backed scope evidence used to distinguish shared gates from independent model pools. */
	readonly scope?: "shared" | "model" | "unknown";
	readonly window_minutes?: number;
}

/**
 * Reports provider-account authentication and quota usage without starting a
 * run. `windows` is empty when the account is unauthenticated or the provider
 * exposes no quota surface.
 *
 * @since 0.6.0
 */
export interface EngineAccountUsage {
	/** The provider account's email when the transport discloses one. */
	readonly account_email?: string;
	readonly authentication: EngineAuthReadiness;
	readonly windows: ReadonlyArray<EngineQuotaWindow>;
}

/** Defines the dependency-free provider-neutral seam implemented by every engine adapter. @since 0.2.0 */
export interface Engine {
	readonly Descriptor: EngineDescriptor;
	/**
	 * Decides whether a source token may continue natively with the requested
	 * model. Matching engine identifiers alone are never sufficient.
	 */
	readonly CheckNativeContinuation?: (
		input: EngineNativeContinuationInput,
	) => Effect.Effect<EngineNativeContinuationDecision, EngineFailure>;
	readonly Open: (input: EngineOpenInput) => Effect.Effect<EngineRun, EngineFailure, Scope.Scope>;
	readonly Probe: (input: EngineProbeInput) => Effect.Effect<EngineProbe, EngineFailure>;
	/**
	 * Reports provider-account quota usage when the adapter supports it. Absent
	 * on adapters whose provider exposes no account-usage surface; the effect is
	 * non-billable and must not start a run.
	 *
	 * @since 0.6.0
	 */
	readonly Usage?: Effect.Effect<EngineAccountUsage, EngineFailure>;
}
