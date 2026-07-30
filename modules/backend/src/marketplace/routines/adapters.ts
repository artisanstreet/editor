import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	EngineCompatibility,
	MarketplacePermission,
	MarketplaceScope,
	MarketplaceTrust,
	NpxSkillsDiscoveryRequest,
	NpxSkillsDiscoveryResult,
	RoutineCommand,
	RoutineFile,
	RoutineSource,
	RoutineDetail,
} from "@artisan/protocol";

/** A fully inspected import candidate. It is intentionally not a persisted Routine. */
export interface RoutineInspection {
	readonly artifact_refs: ReadonlyArray<string>;
	readonly author?: string;
	readonly candidate_id: string;
	readonly compatibility: ReadonlyArray<EngineCompatibility>;
	readonly content_hashes: Readonly<Record<string, string>>;
	readonly description: string;
	readonly display_name: string;
	readonly exported_commands: ReadonlyArray<RoutineCommand>;
	readonly files: ReadonlyArray<RoutineFile>;
	readonly instructions: string;
	readonly permissions: ReadonlyArray<MarketplacePermission>;
	readonly rollback_available: boolean;
	readonly source: RoutineSource;
	readonly trust: MarketplaceTrust;
	readonly version: string;
}

export const RoutineInspectionSchema = Schema.Struct({
	artifact_refs: Schema.Array(Schema.NonEmptyString),
	author: Schema.optional(Schema.NonEmptyString),
	candidate_id: Schema.NonEmptyString,
	compatibility: Schema.Array(EngineCompatibility),
	content_hashes: Schema.Record(Schema.String, Schema.NonEmptyString),
	description: Schema.NonEmptyString,
	display_name: Schema.NonEmptyString,
	exported_commands: Schema.Array(RoutineCommand),
	files: Schema.Array(RoutineFile),
	instructions: Schema.NonEmptyString,
	permissions: Schema.Array(MarketplacePermission),
	rollback_available: Schema.Boolean,
	source: RoutineSource,
	trust: MarketplaceTrust,
	version: Schema.NonEmptyString,
});

/** Redacted discovery error; implementations must not return source contents in this error. */
export class RoutineInspectorError extends Data.TaggedError("RoutineInspectorError")<{
	readonly code: "invalid_source" | "not_found" | "read_failed" | "unsupported";
}> {}

/** Pure discovery boundary. Inspecting a source must never install, start, or connect anything. */
export class RoutineSourceInspector extends Context.Service<
	RoutineSourceInspector,
	{
		readonly Inspect: (input: {
			readonly scope: MarketplaceScope;
			readonly source: RoutineSource;
		}) => Effect.Effect<RoutineInspection, RoutineInspectorError>;
	}
>()("Artisan/Marketplace/RoutineSourceInspector") {}

/** Isolates the evolving `npx skills` output format; it never creates canonical routines or writes files. */
export class NpxSkillsAdapter extends Context.Service<
	NpxSkillsAdapter,
	{
		readonly Discover: (
			input: NpxSkillsDiscoveryRequest,
		) => Effect.Effect<NpxSkillsDiscoveryResult, RoutineInspectorError>;
	}
>()("Artisan/Marketplace/NpxSkillsAdapter") {}

/** Atomic installation boundary invoked only after a matching durable approval. */
export interface RoutineInstallReceipt {
	readonly artifact_refs: ReadonlyArray<string>;
	readonly rollback_id?: string;
}
export const RoutineInstallReceiptSchema = Schema.Struct({
	artifact_refs: Schema.Array(Schema.NonEmptyString),
	rollback_id: Schema.optional(Schema.NonEmptyString),
});
export class RoutineInstallerError extends Data.TaggedError("RoutineInstallerError")<{
	readonly code: "conflict" | "install_failed" | "rollback_failed";
}> {}
export class RoutineInstaller extends Context.Service<
	RoutineInstaller,
	{
		/** Exactly idempotent by operation_id, including a crash after the external effect. */
		readonly Install: (input: {
			readonly inspection: RoutineInspection;
			readonly operation_id: string;
			readonly scope: MarketplaceScope;
		}) => Effect.Effect<RoutineInstallReceipt, RoutineInstallerError>;
		readonly Rollback: (input: {
			/** Stable durable operation key; retries must return without repeating the external effect. */
			readonly operation_id: string;
			readonly rollback_id: string;
		}) => Effect.Effect<void, RoutineInstallerError>;
	}
>()("Artisan/Marketplace/RoutineInstaller") {}

/** Provider mirrors are explicitly synchronized; unknown engines are runtime-only. */
export type RoutineMirrorMode = "native" | "runtime_only" | "unsupported";
export interface RoutineMirrorAdapter {
	readonly engine_id: string;
	readonly mode: RoutineMirrorMode;
	/** Exactly idempotent by operation_id. */
	readonly Sync: (input: {
		readonly operation_id: string;
		readonly routine: RoutineDetail;
	}) => Effect.Effect<{ readonly revision?: string }, RoutineInstallerError>;
	/**
	 * Executes an explicit provider reconciliation. This operation is exactly idempotent by
	 * `operation_id`, including recovery after an external provider effect has succeeded but
	 * before Artisan persists the completion. Import returns the new canonical record.
	 */
	readonly ResolveDrift: (input: {
		readonly action: "ignore" | "import" | "overwrite";
		readonly observed_revision: string;
		readonly operation_id: string;
		readonly routine: RoutineDetail;
	}) => Effect.Effect<
		{ readonly imported?: RoutineDetail; readonly revision?: string },
		RoutineInstallerError
	>;
}

export const RoutineMirrorSyncResult = Schema.Struct({
	revision: Schema.optional(Schema.NonEmptyString),
});
export const RoutineMirrorDriftResult = Schema.Struct({
	imported: Schema.optional(RoutineDetail),
	revision: Schema.optional(Schema.NonEmptyString),
});
export class RoutineMirrorRegistry extends Context.Service<
	RoutineMirrorRegistry,
	{ readonly Find: (engine_id: string) => RoutineMirrorAdapter | undefined }
>()("Artisan/Marketplace/RoutineMirrorRegistry") {}

export const EmptyRoutineMirrorRegistryLive = Layer.succeed(RoutineMirrorRegistry, {
	Find: () => undefined,
});
