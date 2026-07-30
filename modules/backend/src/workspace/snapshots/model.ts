import { Context, Data, Effect, Schema } from "effect";

import { ContentIdentity, Identifier, workspace_text_maximum_bytes } from "@artisan/protocol";

const SnapshotBytes = Schema.Uint8Array.check(
	Schema.makeFilter<Uint8Array>((bytes) =>
		bytes.byteLength <= workspace_text_maximum_bytes
			? undefined
			: `Expected at most ${workspace_text_maximum_bytes} bytes`,
	),
);

export const StageInput = Schema.Struct({
	change_id: Identifier,
	content: SnapshotBytes,
	expected_identity: ContentIdentity,
	thread_id: Identifier,
});

export const ReadInput = Schema.Struct({
	change_id: Identifier,
	expected_identity: ContentIdentity,
	thread_id: Identifier,
});
export const ResumeInput = ReadInput;

export const ConsumeInput = Schema.Struct({
	change_id: Identifier,
	rollback_message_id: Identifier,
	thread_id: Identifier,
});

export const ExistsInput = Schema.Struct({
	change_id: Identifier,
	thread_id: Identifier,
});
export const DiscardRejectedReplaceInput = Schema.Struct({
	change_id: Identifier,
	expected_identity: ContentIdentity,
	replace_message_id: Identifier,
	thread_id: Identifier,
});
export const ContentHash = /^[0-9a-f]{64}$/;

export type ReplaceLifecycle = "applied" | "claimed" | "committed";

/** Supplies bytes and their declared identity for one staged rollback snapshot. */
export type WorkspaceSnapshotStageInput = typeof StageInput.Type;

/** Supplies the expected identity required to read one rollback snapshot. */
export type WorkspaceSnapshotReadInput = typeof ReadInput.Type;

/** Supplies the expected identity required to resume an uncommitted replacement. */
export type WorkspaceSnapshotResumeInput = typeof ResumeInput.Type;

/** Supplies the thread authority required to permanently consume one snapshot. */
export type WorkspaceSnapshotConsumeInput = typeof ConsumeInput.Type;

/** Supplies the thread authority required to inspect snapshot availability. */
export type WorkspaceSnapshotExistsInput = typeof ExistsInput.Type;

/** Selects a rejected replacement whose private rollback snapshot should be discarded. */
export type WorkspaceSnapshotDiscardRejectedReplaceInput = typeof DiscardRejectedReplaceInput.Type;

/** Reports malformed snapshot-store input before it reaches private storage. */
export class WorkspaceSnapshotStoreInvalid extends Data.TaggedError(
	"WorkspaceSnapshotStoreInvalid",
)<{
	readonly change_id?: string;
	readonly operation:
		| "consume"
		| "discard_rejected_replace"
		| "exists"
		| "read"
		| "resume"
		| "stage";
}> {}

/** Reports a change ID already bound to a different available snapshot. */
export class WorkspaceSnapshotStoreConflict extends Data.TaggedError(
	"WorkspaceSnapshotStoreConflict",
)<{
	readonly change_id: string;
	readonly operation: "stage";
}> {}

/** Reports a missing, consumed, corrupt, or otherwise unavailable snapshot. */
export class WorkspaceSnapshotStoreUnavailable extends Data.TaggedError(
	"WorkspaceSnapshotStoreUnavailable",
)<{
	readonly change_id?: string;
	readonly operation:
		| "consume"
		| "discard_rejected_replace"
		| "exists"
		| "read"
		| "resume"
		| "stage";
}> {}

/** Represents failures returned by private workspace rollback snapshot operations. */
export type WorkspaceSnapshotStoreError =
	| WorkspaceSnapshotStoreConflict
	| WorkspaceSnapshotStoreInvalid
	| WorkspaceSnapshotStoreUnavailable;

/** Owns opaque private rollback snapshots independently of the workspace journal. */
export class WorkspaceSnapshotStore extends Context.Service<
	WorkspaceSnapshotStore,
	{
		readonly Consume: (
			input: WorkspaceSnapshotConsumeInput,
		) => Effect.Effect<void, WorkspaceSnapshotStoreError>;
		readonly DiscardRejectedReplace: (
			input: WorkspaceSnapshotDiscardRejectedReplaceInput,
		) => Effect.Effect<void, WorkspaceSnapshotStoreError>;
		readonly Exists: (
			input: WorkspaceSnapshotExistsInput,
		) => Effect.Effect<boolean, WorkspaceSnapshotStoreError>;
		readonly Read: (
			input: WorkspaceSnapshotReadInput,
		) => Effect.Effect<Uint8Array, WorkspaceSnapshotStoreError>;
		readonly Resume: (
			input: WorkspaceSnapshotResumeInput,
		) => Effect.Effect<Uint8Array, WorkspaceSnapshotStoreError>;
		readonly Stage: (
			input: WorkspaceSnapshotStageInput,
		) => Effect.Effect<{ readonly status: "existing" | "staged" }, WorkspaceSnapshotStoreError>;
	}
>()("Artisan/WorkspaceSnapshotStore") {}
