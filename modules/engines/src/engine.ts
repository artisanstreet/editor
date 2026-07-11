import { Data, Effect, Scope, Stream } from "effect";

/** Describes the maturity of one provider capability. @since 0.2.0 */
export type EngineCapabilityState = "supported" | "experimental" | "unsupported";

/** Names a capability declared by an engine adapter. @since 0.2.0 */
export type EngineCapabilityName =
	| "approval"
	| "auth"
	| "cancel"
	| "close"
	| "events"
	| "model_selection"
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

/** Supplies the caller-owned context shared by started and resumed runs. @since 0.2.0 */
export interface EngineRunContext extends EngineRunMetadata {
	readonly artisan_run_id: string;
	readonly working_directory: string;
}

/** Carries provider-owned state that can reopen a run without inventing a checkpoint. @since 0.2.0 */
export interface EngineResumeToken {
	readonly native_thread_id: string;
	readonly opaque_checkpoint?: string;
}

/** Opens a new native thread from initial user text. @since 0.2.0 */
export interface EngineStartInput extends EngineRunContext {
	readonly _tag: "start";
	readonly initial_text: string;
}

/** Reopens provider-owned state and may continue it with new user text. @since 0.2.0 */
export interface EngineResumeInput extends EngineRunContext {
	readonly _tag: "resume";
	readonly next_text?: string;
	readonly resume_token: EngineResumeToken;
}

/** Selects whether a run starts fresh or resumes provider-owned state. @since 0.2.0 */
export type EngineOpenInput = EngineResumeInput | EngineStartInput;

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

/** Streams a partial agent-authored message. @since 0.2.0 */
export interface EngineAgentMessageDeltaObservation extends EngineObservationBase {
	readonly _tag: "agent_message_delta";
	readonly delta: string;
	readonly turn_id: string;
}

/** Completes one agent-authored message. @since 0.2.0 */
export interface EngineAgentMessageCompletedObservation extends EngineObservationBase {
	readonly _tag: "agent_message_completed";
	readonly message: string;
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
	readonly path: string;
}

/** Reports a search operation performed during a run. @since 0.2.0 */
export interface EngineSearchObservation extends EngineObservationBase {
	readonly _tag: "search";
	readonly query: string;
	readonly result_count?: number;
	readonly state: "started" | "completed";
}

/** Reports a provider-native action that has no canonical tool equivalent. @since 0.2.0 */
export interface EngineNativeActionObservation extends EngineObservationBase {
	readonly _tag: "native_action";
	readonly action: string;
	readonly detail?: string;
}

/** Reports an approval request or its resolution. @since 0.2.0 */
export interface EngineApprovalObservation extends EngineObservationBase {
	readonly _tag: "approval";
	readonly approval_id: string;
	readonly approved?: boolean;
	readonly description: string;
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

/** Reports provider usage measured for the run or one turn. @since 0.2.0 */
export interface EngineUsageObservation extends EngineObservationBase {
	readonly _tag: "usage";
	readonly input_tokens?: number;
	readonly output_tokens?: number;
	readonly turn_id?: string;
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
	| EngineReasoningSummaryDeltaObservation
	| EngineRunStateObservation
	| EngineRunTerminalObservation
	| EngineSearchObservation
	| EngineTerminalActivityObservation
	| EngineToolObservation
	| EngineTurnStateObservation
	| EngineUsageObservation;

/** Steers an active run with new user text. @since 0.2.0 */
export interface EngineSteerCommand {
	readonly _tag: "steer";
	readonly command_id: string;
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

/** Defines the dependency-free provider-neutral seam implemented by every engine adapter. @since 0.2.0 */
export interface Engine {
	readonly Descriptor: EngineDescriptor;
	readonly Open: (input: EngineOpenInput) => Effect.Effect<EngineRun, EngineFailure, Scope.Scope>;
	readonly Probe: (input: EngineProbeInput) => Effect.Effect<EngineProbe, EngineFailure>;
}
