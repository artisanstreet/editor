import { Data, Effect } from "effect";

/** Describes whether an engine operation is ready for product use. @since 0.1.0 */
export type EngineCapabilityState = "supported" | "unsupported" | "experimental";

/** Names an operation exposed by an engine adapter. @since 0.1.0 */
export type EngineOperation =
	| "approval"
	| "cancel"
	| "close"
	| "inspect"
	| "resume"
	| "start"
	| "steer";

/** Lists the declared maturity of every engine operation. @since 0.1.0 */
export interface EngineCapabilities {
	readonly approval: EngineCapabilityState;
	readonly cancel: EngineCapabilityState;
	readonly close: EngineCapabilityState;
	readonly inspect: EngineCapabilityState;
	readonly resume: EngineCapabilityState;
	readonly start: EngineCapabilityState;
	readonly steer: EngineCapabilityState;
}

/** Identifies an installed engine adapter and its contract surface. @since 0.1.0 */
export interface EngineDescriptor {
	readonly capabilities: EngineCapabilities;
	readonly display_name: string;
	readonly id: string;
	readonly transport: string;
}

/** Represents an engine run owned by a concrete provider. @since 0.1.0 */
export interface EngineRun {
	readonly engine_id: string;
	readonly run_id: string;
}

/** Supplies optional caller metadata for a non-billable engine inspection. @since 0.1.0 */
export interface EngineInspectInput {
	readonly client_name?: string;
	readonly client_version?: string;
}

/** Supplies a request to start a new provider-owned run. @since 0.1.0 */
export interface EngineStartInput {
	readonly working_directory: string;
}

/** Supplies a request to resume an existing provider-owned run. @since 0.1.0 */
export interface EngineResumeInput {
	readonly run_id: string;
}

/** Supplies a steering instruction for an active provider-owned run. @since 0.1.0 */
export interface EngineSteerInput {
	readonly instruction: string;
	readonly run: EngineRun;
}

/** Supplies a response to an approval requested by an engine run. @since 0.1.0 */
export interface EngineApprovalInput {
	readonly approval_id: string;
	readonly approved: boolean;
	readonly run: EngineRun;
}

/** Supplies a cancellation request for an active engine run. @since 0.1.0 */
export interface EngineCancelInput {
	readonly run: EngineRun;
}

/** Supplies a request to close a provider-owned engine run. @since 0.1.0 */
export interface EngineCloseInput {
	readonly run: EngineRun;
}

/** Reports version and transport facts discovered during inspection. @since 0.1.0 */
export interface EngineInspection {
	readonly descriptor: EngineDescriptor;
	readonly metadata: Readonly<Record<string, string>>;
	readonly version: string;
}

/** Represents an unavailable executable or provider connection. @since 0.1.0 */
export class EngineUnavailableError extends Data.TaggedError("EngineUnavailableError")<{
	readonly engine_id: string;
	readonly message: string;
}> {}

/** Represents an operation deliberately outside this adapter slice. @since 0.1.0 */
export class EngineUnsupportedOperationError extends Data.TaggedError(
	"EngineUnsupportedOperationError",
)<{
	readonly engine_id: string;
	readonly operation: EngineOperation;
}> {}

/** Represents an invalid or incompatible provider transport message. @since 0.1.0 */
export class EngineProtocolError extends Data.TaggedError("EngineProtocolError")<{
	readonly engine_id: string;
	readonly message: string;
}> {}

/** Represents a process boundary failure while operating an engine. @since 0.1.0 */
export class EngineProcessError extends Data.TaggedError("EngineProcessError")<{
	readonly cause: unknown;
	readonly operation: "close" | "exit" | "kill" | "spawn" | "write";
}> {}

/** Represents a duplicate or unknown engine registration. @since 0.1.0 */
export class EngineRegistryError extends Data.TaggedError("EngineRegistryError")<{
	readonly engine_id: string;
	readonly reason: "duplicate_id" | "not_found";
}> {}

/** Enumerates failures emitted by engine adapters. @since 0.1.0 */
export type EngineFailure =
	| EngineProcessError
	| EngineProtocolError
	| EngineRegistryError
	| EngineUnavailableError
	| EngineUnsupportedOperationError;

/** Defines the provider-neutral contract implemented by an engine adapter. @since 0.1.0 */
export interface Engine<Requirements = never> {
	readonly Descriptor: EngineDescriptor;
	readonly Approve: (
		input: EngineApprovalInput,
	) => Effect.Effect<void, EngineFailure, Requirements>;
	readonly Cancel: (input: EngineCancelInput) => Effect.Effect<void, EngineFailure, Requirements>;
	readonly Close: (input: EngineCloseInput) => Effect.Effect<void, EngineFailure, Requirements>;
	readonly Inspect: (
		input: EngineInspectInput,
	) => Effect.Effect<EngineInspection, EngineFailure, Requirements>;
	readonly Resume: (
		input: EngineResumeInput,
	) => Effect.Effect<EngineRun, EngineFailure, Requirements>;
	readonly Start: (
		input: EngineStartInput,
	) => Effect.Effect<EngineRun, EngineFailure, Requirements>;
	readonly Steer: (input: EngineSteerInput) => Effect.Effect<void, EngineFailure, Requirements>;
}
