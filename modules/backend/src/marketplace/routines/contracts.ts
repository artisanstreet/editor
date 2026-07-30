import { Data, Effect } from "effect";
import {
	ProviderSyncState,
	RoutineDetail,
	RoutineDriftOverwriteRequest,
	RoutineInstallPreview,
	RoutineSummary,
} from "@artisan/protocol";
import { settings_scope_id, settings_stream_id } from "../../settings/internal-scope";

export const marketplace_routine_thread_id = settings_scope_id("marketplace-routines");
export const marketplace_routine_stream_id = settings_stream_id("marketplace-routines");

export class RoutineRepositoryError extends Data.TaggedError("RoutineRepositoryError")<{
	readonly code: "conflict" | "invariant" | "not_found";
	readonly message: string;
}> {}

export interface RoutineOperation {
	readonly approval_id: string;
	readonly approval_fingerprint: string;
	readonly operation_id: string;
	readonly preview_json: string;
	readonly request_fingerprint: string;
	readonly routine_id: string;
}

export interface RoutineInstallRequestRecord extends RoutineOperation {}

export interface RoutineDriftOverwriteRequestRecord {
	readonly operation_id: string;
	readonly request: RoutineDriftOverwriteRequest;
}

export interface RoutineRecovery {
	readonly operation_id: string;
	readonly routine_id: string;
	readonly state: "approved" | "installed";
}

export interface RoutineRollbackRecovery {
	readonly operation_id: string;
	readonly rollback_id: string;
	readonly routine_id: string;
}

export interface RoutineMirrorOperation {
	readonly engine_id: string;
	readonly intent_fingerprint: string;
	readonly kind: "drift" | "sync";
	readonly operation_id: string;
	readonly routine_id: string;
}

export type RoutineOperationClaim =
	| { readonly _tag: "Claimed" }
	| { readonly _tag: "InFlight" }
	| { readonly _tag: "Completed"; readonly journal_sequence: number };

export const mirror_operation_lease_milliseconds = 60_000;

export interface RoutineRepositoryApi {
	readonly ReadSummaries: Effect.Effect<ReadonlyArray<RoutineSummary>, RoutineRepositoryError>;
	readonly ReadDetail: (
		routine_id: string,
	) => Effect.Effect<RoutineDetail, RoutineRepositoryError>;
	/** Records a preview request without granting authority to write. */
	readonly RecordPendingInstall: (
		operation: RoutineInstallRequestRecord,
	) => Effect.Effect<"accepted" | "duplicate", RoutineRepositoryError>;
	/** Resolves a decision from durable approval state, independent of a renderer connection. */
	readonly ReadPendingInstall: (
		approval_id: string,
	) => Effect.Effect<
		RoutineInstallRequestRecord & { readonly preview: RoutineInstallPreview },
		RoutineRepositoryError
	>;
	readonly RecordPendingDriftOverwrite: (
		input: RoutineDriftOverwriteRequestRecord,
	) => Effect.Effect<"accepted" | "duplicate", RoutineRepositoryError>;
	readonly ReadPendingDriftOverwrite: (
		approval_id: string,
	) => Effect.Effect<RoutineDriftOverwriteRequestRecord, RoutineRepositoryError>;
	readonly DecideDriftOverwrite: (input: {
		readonly approval_id: string;
		readonly approved: boolean;
		readonly intent_fingerprint: string;
	}) => Effect.Effect<"approved" | "denied" | "resume", RoutineRepositoryError>;
	/** The only transition that makes an installer eligible to run. */
	readonly DecideInstall: (input: {
		readonly approval_fingerprint: string;
		readonly approval_id: string;
		readonly approved: boolean;
		readonly operation_id: string;
	}) => Effect.Effect<"approved" | "denied" | "installed" | "resume", RoutineRepositoryError>;
	readonly CommitInstalled: (input: {
		readonly artifact_refs: ReadonlyArray<string>;
		readonly detail: RoutineDetail;
		readonly operation_id: string;
		readonly rollback_json?: string;
	}) => Effect.Effect<number, RoutineRepositoryError>;
	/** Records a terminal installer failure in the canonical lifecycle ledger. */
	readonly RecordInstallFailure: (input: {
		readonly code: "conflict" | "install_failed" | "rollback_failed";
		readonly operation_id: string;
	}) => Effect.Effect<number, RoutineRepositoryError>;
	readonly Transition: (input: {
		readonly enabled: boolean;
		readonly operation: "enabled" | "disabled" | "invoked" | "removed";
		readonly operation_id: string;
		readonly routine_id: string;
		readonly status: RoutineDetail["status"];
		readonly tool_name?: string;
	}) => Effect.Effect<number, RoutineRepositoryError>;
	readonly ReadRecovery: Effect.Effect<ReadonlyArray<RoutineRecovery>, RoutineRepositoryError>;
	readonly ClaimRollback: (
		input: RoutineRollbackRecovery,
	) => Effect.Effect<"claimed" | "completed", RoutineRepositoryError>;
	readonly CommitRollback: (
		operation_id: string,
	) => Effect.Effect<number, RoutineRepositoryError>;
	readonly ReadRollbackRecovery: Effect.Effect<
		ReadonlyArray<RoutineRollbackRecovery>,
		RoutineRepositoryError
	>;
	readonly ClaimMirrorOperation: (
		input: RoutineMirrorOperation,
	) => Effect.Effect<RoutineOperationClaim, RoutineRepositoryError>;
	readonly CommitMirrorOperation: (input: {
		readonly imported?: RoutineDetail;
		readonly operation_id: string;
		readonly state: ProviderSyncState;
	}) => Effect.Effect<number, RoutineRepositoryError>;
	/** Releases a failed adapter attempt without permitting another runtime's lease to be cleared. */
	readonly ReleaseMirrorOperation: (
		operation_id: string,
	) => Effect.Effect<void, RoutineRepositoryError>;
}
