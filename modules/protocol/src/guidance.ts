import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";

const text_encoder = new TextEncoder();

/** Defines the maximum raw and canonical size of global guidance content. */
export const global_guidance_maximum_bytes = 1_048_576;

/** Normalizes BOM, line endings, and the terminal newline used by canonical guidance. */
export function normalize_global_guidance_content(content: string) {
	const without_bom = content.startsWith("\uFEFF") ? content.slice(1) : content;
	const normalized = without_bom.replace(/\r\n?/g, "\n");

	return normalized.length === 0 || normalized.endsWith("\n") ? normalized : `${normalized}\n`;
}

/** Bounds the normalized UTF-8 byte size represented by guidance metadata. */
export const GuidanceByteCount = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0),
	Schema.isLessThanOrEqualTo(global_guidance_maximum_bytes),
);

/** Validates the SHA-256 digest used to identify content without carrying it. */
export const GuidanceHash = Schema.String.check(
	Schema.isPattern(/^[a-f0-9]{64}$/, { message: "Expected a lowercase SHA-256 hash" }),
);

/** Identifies the V1 engines whose guidance behavior is explicitly integrated. */
export const GlobalGuidanceProvider = Schema.Literals(["claude", "codex"]);

export type GlobalGuidanceProvider = typeof GlobalGuidanceProvider.Type;

/** Describes one provider's reconciliation state without exposing its guidance content. */
export const GuidanceProviderSyncStatus = Schema.Literals([
	"synced",
	"applied_at_run_time",
	"unsupported",
	"sync_failed",
	"drift_detected",
	"awaiting_selection",
]);

export type GuidanceProviderSyncStatus = typeof GuidanceProviderSyncStatus.Type;

/** Records canonical metadata for the one global guidance file. */
export const GlobalGuidanceCanonicalMetadata = Schema.Struct({
	byte_count: Schema.optional(GuidanceByteCount),
	content_hash: Schema.optional(GuidanceHash),
	selected_provider: Schema.optional(GlobalGuidanceProvider),
	status: Schema.Literals(["initialization_required", "selection_required", "ready"]),
	updated_at: IsoDateTime,
});

export type GlobalGuidanceCanonicalMetadata = typeof GlobalGuidanceCanonicalMetadata.Type;

/** Records one provider's metadata-only mirror state and recoverability evidence. */
export const GlobalGuidanceProviderMetadata = Schema.Struct({
	applied_byte_count: Schema.optional(GuidanceByteCount),
	applied_hash: Schema.optional(GuidanceHash),
	backup_path: Schema.optional(Schema.NonEmptyString),
	ignored_drift_hash: Schema.optional(GuidanceHash),
	last_error_code: Schema.optional(Identifier),
	modified_at: Schema.optional(IsoDateTime),
	observed_byte_count: Schema.optional(GuidanceByteCount),
	observed_hash: Schema.optional(GuidanceHash),
	path: Schema.optional(Schema.NonEmptyString),
	provider: GlobalGuidanceProvider,
	status: GuidanceProviderSyncStatus,
	updated_at: IsoDateTime,
});

export type GlobalGuidanceProviderMetadata = typeof GlobalGuidanceProviderMetadata.Type;

/** Accepts a filesystem reconciliation outcome without carrying provider guidance content. */
export const GlobalGuidanceProviderReconciliation = Schema.Struct({
	applied_byte_count: Schema.optional(GuidanceByteCount),
	applied_hash: Schema.optional(GuidanceHash),
	backup_path: Schema.optional(Schema.NonEmptyString),
	ignored_drift_hash: Schema.optional(GuidanceHash),
	last_error_code: Schema.optional(Identifier),
	modified_at: Schema.optional(IsoDateTime),
	observed_byte_count: Schema.optional(GuidanceByteCount),
	observed_hash: Schema.optional(GuidanceHash),
	path: Schema.optional(Schema.NonEmptyString),
	provider: GlobalGuidanceProvider,
	status: GuidanceProviderSyncStatus,
});

export type GlobalGuidanceProviderReconciliation = typeof GlobalGuidanceProviderReconciliation.Type;

/** Supplies the content-free global guidance projection persisted in SQLite. */
export const GlobalGuidanceMetadata = Schema.Struct({
	canonical: GlobalGuidanceCanonicalMetadata,
	providers: Schema.Array(GlobalGuidanceProviderMetadata),
});

export type GlobalGuidanceMetadata = typeof GlobalGuidanceMetadata.Type;

/** Bounds canonical guidance carried only over the local control channel and filesystem. */
export const GlobalGuidanceContent = Schema.String.check(
	Schema.makeFilter<string>((value) => {
		const raw_byte_count = text_encoder.encode(value).byteLength;
		const canonical_byte_count = text_encoder.encode(
			normalize_global_guidance_content(value),
		).byteLength;

		return raw_byte_count <= global_guidance_maximum_bytes &&
			canonical_byte_count <= global_guidance_maximum_bytes
			? undefined
			: `Expected at most ${global_guidance_maximum_bytes} UTF-8 bytes before and after normalization`;
	}),
);

/** Selects one freshly rediscovered provider value during first-run initialization. */
export const GlobalGuidanceSelectionRequest = Schema.Struct({
	content_hash: GuidanceHash,
	provider: GlobalGuidanceProvider,
});

export type GlobalGuidanceSelectionRequest = typeof GlobalGuidanceSelectionRequest.Type;

/** Carries a user-authored canonical replacement without journaling its content. */
export const GlobalGuidanceUpdateRequest = Schema.Struct({
	content: GlobalGuidanceContent,
});

export type GlobalGuidanceUpdateRequest = typeof GlobalGuidanceUpdateRequest.Type;

/** Resolves one exact observed drift value without silently merging provider content. */
export const GlobalGuidanceDriftResolutionRequest = Schema.Struct({
	action: Schema.Literals(["import", "overwrite", "ignore"]),
	observed_hash: GuidanceHash,
	provider: GlobalGuidanceProvider,
});

export type GlobalGuidanceDriftResolutionRequest = typeof GlobalGuidanceDriftResolutionRequest.Type;

/** Requests another provider mirror attempt without exposing provider-specific controls. */
export const GlobalGuidanceRetryRequest = Schema.Struct({
	provider: GlobalGuidanceProvider,
});

export type GlobalGuidanceRetryRequest = typeof GlobalGuidanceRetryRequest.Type;

/** Exposes a first-run candidate only from freshly discovered provider files. */
export const GlobalGuidanceCandidate = Schema.Struct({
	byte_count: GuidanceByteCount,
	content_hash: GuidanceHash,
	modified_at: IsoDateTime,
	path: Schema.NonEmptyString,
	preview: GlobalGuidanceContent,
	provider: GlobalGuidanceProvider,
});

export type GlobalGuidanceCandidate = typeof GlobalGuidanceCandidate.Type;

/** Returns canonical content with metadata and any unresolved first-run choices. */
export const GlobalGuidanceSnapshot = Schema.Struct({
	candidates: Schema.Array(GlobalGuidanceCandidate),
	content: GlobalGuidanceContent,
	metadata: GlobalGuidanceMetadata,
});

export type GlobalGuidanceSnapshot = typeof GlobalGuidanceSnapshot.Type;

/** Identifies why the backend committed canonical file metadata after a file write. */
export const GlobalGuidanceCommitReason = Schema.Literals([
	"first_run",
	"user_update",
	"selection",
	"drift_import",
	"recovery",
]);

export type GlobalGuidanceCommitReason = typeof GlobalGuidanceCommitReason.Type;

/** Carries the content-free intent committed only after the canonical file is durable. */
export const GlobalGuidanceCanonicalCommitIntent = Schema.Struct({
	byte_count: GuidanceByteCount,
	content_hash: GuidanceHash,
	reason: GlobalGuidanceCommitReason,
	selected_provider: Schema.optional(GlobalGuidanceProvider),
	type: Schema.Literal("guidance.canonical.commit"),
});

export type GlobalGuidanceCanonicalCommitIntent = typeof GlobalGuidanceCanonicalCommitIntent.Type;

/** Records first-run ambiguity without persisting any candidate content. */
export const GlobalGuidanceSelectionRequiredIntent = Schema.Struct({
	candidate_hashes: Schema.NonEmptyArray(GuidanceHash),
	type: Schema.Literal("guidance.selection.require"),
});

export type GlobalGuidanceSelectionRequiredIntent =
	typeof GlobalGuidanceSelectionRequiredIntent.Type;

/** Records a metadata-only canonical guidance transition in the durable journal. */
export const GlobalGuidanceCanonicalUpdatedEvent = Schema.Struct({
	byte_count: GuidanceByteCount,
	content_hash: GuidanceHash,
	selected_provider: Schema.optional(GlobalGuidanceProvider),
	type: Schema.Literal("guidance.canonical.updated"),
});

export type GlobalGuidanceCanonicalUpdatedEvent = typeof GlobalGuidanceCanonicalUpdatedEvent.Type;

/** Records that first-run provider values require an explicit source choice. */
export const GlobalGuidanceSelectionRequiredEvent = Schema.Struct({
	candidate_hashes: Schema.NonEmptyArray(GuidanceHash),
	type: Schema.Literal("guidance.selection.required"),
});

export type GlobalGuidanceSelectionRequiredEvent = typeof GlobalGuidanceSelectionRequiredEvent.Type;

/** Records one provider mirror state transition without including native guidance. */
export const GlobalGuidanceProviderReconciledEvent = Schema.Struct({
	applied_byte_count: Schema.optional(GuidanceByteCount),
	applied_hash: Schema.optional(GuidanceHash),
	ignored_drift_hash: Schema.optional(GuidanceHash),
	last_error_code: Schema.optional(Identifier),
	observed_byte_count: Schema.optional(GuidanceByteCount),
	observed_hash: Schema.optional(GuidanceHash),
	provider: GlobalGuidanceProvider,
	status: GuidanceProviderSyncStatus,
	type: Schema.Literal("guidance.provider.reconciled"),
});

export type GlobalGuidanceProviderReconciledEvent =
	typeof GlobalGuidanceProviderReconciledEvent.Type;
