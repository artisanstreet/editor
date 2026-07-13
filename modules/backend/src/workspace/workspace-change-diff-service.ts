import { and, eq } from "drizzle-orm";
import { FILE_HEADERS_ONLY, formatPatch, structuredPatch, type StructuredPatch } from "diff";
import { Context, Crypto, Data, Effect, Encoding, Layer, Schema } from "effect";

import {
	ContentIdentity,
	Identifier,
	WorkspaceChangeDiffQuery,
	WorkspaceChangeDiffQueryResult,
	WorkspacePath,
	workspace_diff_context_lines,
	workspace_diff_format_version,
	workspace_diff_maximum_bytes,
	workspace_diff_maximum_lines_per_side,
	workspace_diff_maximum_rendered_lines,
	workspace_text_maximum_bytes,
	type ContentIdentity as ContentIdentityValue,
	type WorkspaceChangeDiffQueryResult as WorkspaceChangeDiffQueryResultValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import {
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeDiffs,
	WorkspaceChangeOperations,
	WorkspaceChanges,
} from "../persistence/schema";
import { workspace_diff_patch_matches_path } from "./workspace-change-diff-format";

/** Caps V1 edit-distance search even for tiny inputs. */
export const workspace_diff_v1_maximum_edit_length = 16_384;

/** Caps V1 edit-distance work across both line streams before the fixed ceiling applies. */
export const workspace_diff_v1_complexity_budget = 4_000_000;

const SourceBytes = Schema.Uint8Array.check(
	Schema.makeFilter<Uint8Array>((bytes) =>
		bytes.byteLength <= workspace_text_maximum_bytes
			? undefined
			: `Expected at most ${workspace_text_maximum_bytes} bytes`,
	),
);

const PatchBytes = Schema.Uint8Array.check(
	Schema.makeFilter<Uint8Array>((bytes) =>
		bytes.byteLength <= workspace_diff_maximum_bytes
			? undefined
			: `Expected at most ${workspace_diff_maximum_bytes} bytes`,
	),
);

export const PreparedWorkspaceChangeDiff = Schema.Struct({
	added_line_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(workspace_diff_maximum_lines_per_side),
	),
	after_identity: ContentIdentity,
	before_identity: ContentIdentity,
	change_id: Identifier,
	context_lines: Schema.Literal(workspace_diff_context_lines),
	format: Schema.Literal("unified"),
	format_version: Schema.Literal(workspace_diff_format_version),
	message_id: Identifier,
	patch: PatchBytes,
	patch_identity: Schema.Struct({
		algorithm: Schema.Literal("sha256"),
		byte_count: Schema.Int.check(
			Schema.isGreaterThanOrEqualTo(0),
			Schema.isLessThanOrEqualTo(workspace_diff_maximum_bytes),
		),
		content_hash: Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/)),
	}),
	path: WorkspacePath,
	removed_line_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(workspace_diff_maximum_lines_per_side),
	),
	thread_id: Identifier,
	workspace_id: Identifier,
}).check(
	Schema.makeFilter<typeof PreparedWorkspaceChangeDiff.Type>((prepared) =>
		prepared.patch.byteLength === prepared.patch_identity.byte_count
			? undefined
			: "Expected patch identity byte count to match the patch",
	),
);

const PrepareInput = Schema.Struct({
	after: SourceBytes,
	after_identity: ContentIdentity,
	before: SourceBytes,
	before_identity: ContentIdentity,
	change_id: Identifier,
	message_id: Identifier,
	path: WorkspacePath,
	thread_id: Identifier,
	workspace_id: Identifier,
});

/** Carries exact private diff bytes validated before a native replacement. */
export type PreparedWorkspaceChangeDiff = typeof PreparedWorkspaceChangeDiff.Type;

/** Reports invalid, non-UTF-8, or identity-mismatched diff input without returning source bytes. */
export class WorkspaceChangeDiffInvalid extends Data.TaggedError("WorkspaceChangeDiffInvalid")<{
	readonly reason: "identity" | "input" | "utf8";
}> {}

/** Reports deterministic V1 budget exhaustion without returning source bytes. */
export class WorkspaceChangeDiffLimit extends Data.TaggedError("WorkspaceChangeDiffLimit")<{
	readonly limit: "edit_length" | "lines" | "patch_bytes" | "rendered_lines";
}> {}

/** Reports unavailable, erased, or corrupt immutable diff state without returning source bytes. */
export class WorkspaceChangeDiffUnavailable extends Data.TaggedError(
	"WorkspaceChangeDiffUnavailable",
)<{ readonly reason: "erased" | "legacy_unavailable" | "missing" | "corrupt" }> {}

/** Represents failures returned by the immutable workspace diff module. */
export type WorkspaceChangeDiffServiceError =
	| WorkspaceChangeDiffInvalid
	| WorkspaceChangeDiffLimit
	| WorkspaceChangeDiffUnavailable;

function identities_match(left: ContentIdentityValue, right: ContentIdentityValue) {
	return (
		left.algorithm === right.algorithm &&
		left.byte_count === right.byte_count &&
		left.content_hash === right.content_hash
	);
}

function logical_line_count(text: string) {
	if (text.length === 0) {
		return 0;
	}

	const line_breaks = text.match(/\r\n|\r|\n/gu)?.length ?? 0;
	const has_unterminated_line = !text.endsWith("\n") && !text.endsWith("\r");

	return line_breaks + (has_unterminated_line ? 1 : 0);
}

function edit_length_budget(before_lines: number, after_lines: number) {
	const total_lines = Math.max(1, before_lines + after_lines);

	return Math.min(
		workspace_diff_v1_maximum_edit_length,
		Math.floor(workspace_diff_v1_complexity_budget / total_lines),
	);
}

function patch_change_counts(patch: StructuredPatch) {
	return patch.hunks.reduce(
		(counts, hunk) => ({
			added: counts.added + hunk.lines.filter((line) => line.startsWith("+")).length,
			removed: counts.removed + hunk.lines.filter((line) => line.startsWith("-")).length,
		}),
		{ added: 0, removed: 0 },
	);
}

function decode_utf8(bytes: Uint8Array) {
	return Effect.try({
		catch: () => new WorkspaceChangeDiffInvalid({ reason: "utf8" }),
		try: () => new TextDecoder("utf-8", { fatal: true }).decode(bytes),
	});
}

function compute_structured_patch(
	path: string,
	before: string,
	after: string,
	maximum_edit_length: number,
) {
	/** jsdiff's async mode incurs one platform timer per edit-distance step. */
	return Effect.try({
		catch: () => new WorkspaceChangeDiffInvalid({ reason: "input" }),
		try: () =>
			structuredPatch(`a/${path}`, `b/${path}`, before, after, undefined, undefined, {
				context: workspace_diff_context_lines,
				maxEditLength: maximum_edit_length,
			}),
	});
}

/** Owns deterministic preparation and immutable historical reads of workspace change patches. */
export class WorkspaceChangeDiffService extends Context.Service<
	WorkspaceChangeDiffService,
	{
		readonly Prepare: (
			input: typeof PrepareInput.Type,
		) => Effect.Effect<
			PreparedWorkspaceChangeDiff,
			WorkspaceChangeDiffInvalid | WorkspaceChangeDiffLimit
		>;
		readonly Read: (
			query: typeof WorkspaceChangeDiffQuery.Type,
		) => Effect.Effect<WorkspaceChangeDiffQueryResultValue, WorkspaceChangeDiffUnavailable>;
	}
>()("Artisan/WorkspaceChangeDiffService") {}

/** Builds the immutable workspace-diff module from SQLite and Effect Crypto. */
export const WorkspaceChangeDiffServiceLive = Layer.effect(
	WorkspaceChangeDiffService,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const compute_identity = (bytes: Uint8Array) =>
			crypto.digest("SHA-256", bytes).pipe(
				Effect.map((digest) => ({
					algorithm: "sha256" as const,
					byte_count: bytes.byteLength,
					content_hash: Encoding.encodeHex(digest),
				})),
			);
		const decode_identity_json = (value: string) =>
			Effect.try({
				catch: () => new WorkspaceChangeDiffUnavailable({ reason: "corrupt" }),
				try: () => JSON.parse(value),
			}).pipe(
				Effect.flatMap(
					Schema.decodeUnknownEffect(ContentIdentity, { onExcessProperty: "error" }),
				),
				Effect.mapError(() => new WorkspaceChangeDiffUnavailable({ reason: "corrupt" })),
			);
		const Prepare = (
			input: typeof PrepareInput.Type,
		): Effect.Effect<
			PreparedWorkspaceChangeDiff,
			WorkspaceChangeDiffInvalid | WorkspaceChangeDiffLimit
		> =>
			Effect.gen(function* () {
				const decoded = yield* Schema.decodeUnknownEffect(PrepareInput, {
					onExcessProperty: "error",
				})(input);
				const before_bytes = Uint8Array.from(decoded.before);
				const after_bytes = Uint8Array.from(decoded.after);
				const [before_identity, after_identity] = yield* Effect.all([
					compute_identity(before_bytes),
					compute_identity(after_bytes),
				]);

				if (
					!identities_match(before_identity, decoded.before_identity) ||
					!identities_match(after_identity, decoded.after_identity)
				) {
					return yield* new WorkspaceChangeDiffInvalid({ reason: "identity" });
				}

				const [before, after] = yield* Effect.all([
					decode_utf8(before_bytes),
					decode_utf8(after_bytes),
				]);
				const before_lines = logical_line_count(before);
				const after_lines = logical_line_count(after);

				if (
					before_lines > workspace_diff_maximum_lines_per_side ||
					after_lines > workspace_diff_maximum_lines_per_side
				) {
					return yield* new WorkspaceChangeDiffLimit({ limit: "lines" });
				}

				const patch_object = yield* compute_structured_patch(
					decoded.path,
					before,
					after,
					edit_length_budget(before_lines, after_lines),
				);

				if (patch_object === undefined) {
					return yield* new WorkspaceChangeDiffLimit({ limit: "edit_length" });
				}

				const patch_text = formatPatch(patch_object, FILE_HEADERS_ONLY);
				const patch = new TextEncoder().encode(patch_text);

				if (logical_line_count(patch_text) > workspace_diff_maximum_rendered_lines) {
					return yield* new WorkspaceChangeDiffLimit({ limit: "rendered_lines" });
				}

				if (patch.byteLength > workspace_diff_maximum_bytes) {
					return yield* new WorkspaceChangeDiffLimit({ limit: "patch_bytes" });
				}

				const counts = patch_change_counts(patch_object);
				const patch_identity = yield* compute_identity(patch);

				return yield* Schema.decodeUnknownEffect(PreparedWorkspaceChangeDiff, {
					onExcessProperty: "error",
				})({
					added_line_count: counts.added,
					after_identity: decoded.after_identity,
					before_identity: decoded.before_identity,
					change_id: decoded.change_id,
					context_lines: workspace_diff_context_lines,
					format: "unified",
					format_version: workspace_diff_format_version,
					message_id: decoded.message_id,
					patch,
					patch_identity,
					path: decoded.path,
					removed_line_count: counts.removed,
					thread_id: decoded.thread_id,
					workspace_id: decoded.workspace_id,
				});
			}).pipe(
				Effect.mapError((error) =>
					error instanceof WorkspaceChangeDiffInvalid ||
					error instanceof WorkspaceChangeDiffLimit
						? error
						: new WorkspaceChangeDiffInvalid({ reason: "input" }),
				),
			);
		const Read = (
			query: typeof WorkspaceChangeDiffQuery.Type,
		): Effect.Effect<WorkspaceChangeDiffQueryResultValue, WorkspaceChangeDiffUnavailable> =>
			Effect.gen(function* () {
				const decoded = yield* Schema.decodeUnknownEffect(WorkspaceChangeDiffQuery, {
					onExcessProperty: "error",
				})(query);

				return yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [thread] = yield* transaction
							.select({ thread_id: Threads.thread_id })
							.from(Threads)
							.where(eq(Threads.thread_id, decoded.thread_id))
							.limit(1);
						const [claim] = yield* transaction
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(ThreadErasureClaims)
							.where(eq(ThreadErasureClaims.thread_id, decoded.thread_id))
							.limit(1);
						const [tombstone] = yield* transaction
							.select({ thread_id: ThreadTombstones.thread_id })
							.from(ThreadTombstones)
							.where(eq(ThreadTombstones.thread_id, decoded.thread_id))
							.limit(1);

						if (!thread || claim || tombstone) {
							return yield* new WorkspaceChangeDiffUnavailable({ reason: "erased" });
						}

						const [change] = yield* transaction
							.select()
							.from(WorkspaceChanges)
							.where(
								and(
									eq(WorkspaceChanges.change_id, decoded.change_id),
									eq(WorkspaceChanges.thread_id, decoded.thread_id),
								),
							)
							.limit(1);

						if (!change) {
							return yield* new WorkspaceChangeDiffUnavailable({ reason: "missing" });
						}

						if (change.diff_state === "legacy_unavailable") {
							return yield* new WorkspaceChangeDiffUnavailable({
								reason: "legacy_unavailable",
							});
						}

						if (change.diff_state !== "available") {
							return yield* new WorkspaceChangeDiffUnavailable({ reason: "corrupt" });
						}

						const [operation] = yield* transaction
							.select()
							.from(WorkspaceChangeOperations)
							.where(
								eq(WorkspaceChangeOperations.message_id, change.source_command_id),
							)
							.limit(1);
						const [row] = yield* transaction
							.select()
							.from(WorkspaceChangeDiffs)
							.where(eq(WorkspaceChangeDiffs.change_id, decoded.change_id))
							.limit(1);

						if (
							!operation ||
							operation.action !== "replace" ||
							operation.lifecycle !== "committed" ||
							operation.journal_sequence === null ||
							operation.change_id !== change.change_id ||
							operation.thread_id !== change.thread_id ||
							operation.workspace_id !== change.workspace_id ||
							operation.path !== change.path ||
							operation.diff_format_version !== workspace_diff_format_version ||
							!row ||
							row.source_command_id !== change.source_command_id ||
							row.thread_id !== change.thread_id ||
							row.workspace_id !== change.workspace_id ||
							row.path !== change.path
						) {
							return yield* new WorkspaceChangeDiffUnavailable({ reason: "corrupt" });
						}

						const [
							change_before_identity,
							change_after_identity,
							operation_before_identity,
							operation_after_identity,
							row_before_identity,
							row_after_identity,
						] = yield* Effect.all([
							decode_identity_json(change.before_identity_json),
							decode_identity_json(change.after_identity_json),
							decode_identity_json(operation.expected_identity_json ?? ""),
							decode_identity_json(operation.result_identity_json ?? ""),
							decode_identity_json(row.before_identity_json),
							decode_identity_json(row.after_identity_json),
						]);

						if (
							!identities_match(change_before_identity, operation_before_identity) ||
							!identities_match(change_before_identity, row_before_identity) ||
							!identities_match(change_after_identity, operation_after_identity) ||
							!identities_match(change_after_identity, row_after_identity)
						) {
							return yield* new WorkspaceChangeDiffUnavailable({ reason: "corrupt" });
						}

						const patch = Uint8Array.from(row.patch);
						const patch_identity = yield* compute_identity(patch);
						const patch_text = yield* decode_utf8(patch);

						if (
							row.patch_byte_count !== patch.byteLength ||
							row.patch_hash !== patch_identity.content_hash ||
							row.format !== "unified" ||
							row.format_version !== workspace_diff_format_version ||
							row.context_lines !== workspace_diff_context_lines ||
							logical_line_count(patch_text) >
								workspace_diff_maximum_rendered_lines ||
							!workspace_diff_patch_matches_path(patch_text, change.path)
						) {
							return yield* new WorkspaceChangeDiffUnavailable({ reason: "corrupt" });
						}

						return yield* Schema.decodeUnknownEffect(WorkspaceChangeDiffQueryResult, {
							onExcessProperty: "error",
						})({
							added_line_count: row.added_line_count,
							after_identity: row_after_identity,
							before_identity: row_before_identity,
							change_id: row.change_id,
							context_lines: workspace_diff_context_lines,
							format: "unified",
							format_version: workspace_diff_format_version,
							patch: patch_text,
							patch_identity,
							path: row.path,
							removed_line_count: row.removed_line_count,
							thread_id: row.thread_id,
							truncated: false,
							workspace_id: row.workspace_id,
						});
					}),
				);
			}).pipe(
				Effect.mapError((error) =>
					error instanceof WorkspaceChangeDiffUnavailable
						? error
						: new WorkspaceChangeDiffUnavailable({ reason: "corrupt" }),
				),
			);

		return { Prepare, Read };
	}),
);
