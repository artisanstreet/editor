import { and, asc, eq, ne, or } from "drizzle-orm";
import { Context, Crypto, Effect, Encoding, Option, Schema } from "effect";

import {
	DecodeJson,
	bytes_match,
	change_identity_matches,
	command_payload_json,
	event_matches_operation,
	identities_match,
	immutable_operations_match,
	normalize_commit_error,
	normalize_error,
	operation_from_claim,
	raw_origins_match,
} from "./operation";

import {
	ContentIdentity,
	workspace_diff_context_lines,
	workspace_diff_format_version,
	workspace_diff_maximum_bytes,
	workspace_diff_maximum_lines_per_side,
	workspace_diff_maximum_rendered_lines,
	type ContentIdentity as ContentIdentityValue,
	type WorkspaceChange as WorkspaceChangeValue,
} from "@artisan/protocol";

import { Database } from "../../persistence/database";
import { JournalNotifier } from "../../persistence/journal-notifier";
import { IsThreadLive } from "../../persistence/thread-liveness";
import {
	JournalCommands,
	JournalEvents,
	WorkspaceChangeOperations,
	WorkspaceChangeDiffs,
	WorkspaceChanges,
	WorkspaceMutationPayloads,
} from "../../persistence/tables";
import {
	AppendJournalEventInTransaction,
	JournalInvariantError,
} from "../../persistence/journal-store";
import { RuntimeMetadata } from "../../runtime/metadata";
import {
	PreparedWorkspaceChangeDiff as PreparedWorkspaceChangeDiffSchema,
	type PreparedWorkspaceChangeDiff,
} from "./diff";
import { workspace_diff_patch_matches_path } from "./diff-format";
import {
	WorkspaceChangeCommandIdentity,
	WorkspaceChangeTransitionError,
	type WorkspaceChangeOperation,
} from "./model";
import { DecodeChange, DecodeEvent, DecodeOperation, DecodeStoredRawOrigin } from "./storage-codec";

export {
	WorkspaceChangeIdConflict,
	WorkspaceChangeRepository,
	WorkspaceChangeTransitionError,
	type ClaimReplace,
	type ClaimReview,
	type ClaimRollback,
	type ReconcileWorkspaceChange,
	type WorkspaceChangeClaim,
	type WorkspaceChangeCommit,
	type WorkspaceChangeEvent,
	type WorkspaceChangeOperation,
	type WorkspaceChangeReconciliation,
	type WorkspaceChangeRepositoryError,
} from "./model";

/** Supplies the SQLite-backed workspace change repository. */

export const MakeWorkspaceChangeContext = Effect.gen(function* () {
	const crypto = yield* Crypto.Crypto;
	const database = yield* Database;
	const metadata = yield* RuntimeMetadata;
	const notifier = yield* JournalNotifier;
	const ValidatePreparedDiff = (prepared_diff: PreparedWorkspaceChangeDiff | undefined) =>
		Effect.gen(function* () {
			if (prepared_diff === undefined) {
				return yield* new WorkspaceChangeTransitionError({
					message: "Workspace replace requires a prepared diff",
				});
			}

			const decoded = yield* Schema.decodeUnknownEffect(PreparedWorkspaceChangeDiffSchema, {
				onExcessProperty: "error",
			})(prepared_diff);
			const patch = Uint8Array.from(decoded.patch);
			const digest = yield* crypto.digest("SHA-256", patch);
			const validated = { ...decoded, patch };

			if (
				patch.byteLength !== decoded.patch_identity.byte_count ||
				Encoding.encodeHex(digest) !== decoded.patch_identity.content_hash
			) {
				return yield* new WorkspaceChangeTransitionError({
					message: "Workspace prepared diff identity is invalid",
				});
			}

			return validated;
		}).pipe(
			Effect.mapError((error) =>
				error instanceof WorkspaceChangeTransitionError
					? error
					: new WorkspaceChangeTransitionError({
							message: "Workspace prepared diff is invalid",
						}),
			),
		);

	const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
		IsThreadLive(transaction, thread_id).pipe(
			Effect.filterOrFail(
				(live) => live,
				() =>
					new WorkspaceChangeTransitionError({
						message: `Thread ${thread_id} is unavailable for workspace changes`,
					}),
			),
			Effect.asVoid,
		);

	const ReadTransitionChange = (
		transaction: typeof database.client,
		operation: WorkspaceChangeOperation,
	) =>
		Effect.gen(function* () {
			const [stored] = yield* transaction
				.select()
				.from(WorkspaceChanges)
				.where(
					and(
						eq(WorkspaceChanges.change_id, operation.change_id),
						eq(WorkspaceChanges.thread_id, operation.thread_id),
					),
				)
				.limit(1);

			if (!stored) {
				return yield* new WorkspaceChangeTransitionError({
					message: `Workspace change ${operation.change_id} is missing for this thread`,
				});
			}

			return { change: yield* DecodeChange(stored), row: stored };
		});
	const DecodeStoredIdentity = (value: string, message: string) =>
		DecodeJson(value).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity)),
			Effect.mapError(() => new JournalInvariantError({ message })),
		);
	const ValidateStoredReplaceDiff = (
		transaction: typeof database.client,
		operation: Extract<WorkspaceChangeOperation, { readonly action: "replace" }>,
		projection: typeof WorkspaceChanges.$inferSelect,
		prepared_diff?: PreparedWorkspaceChangeDiff,
	) =>
		Effect.gen(function* () {
			const rows = yield* transaction
				.select()
				.from(WorkspaceChangeDiffs)
				.where(
					or(
						eq(WorkspaceChangeDiffs.change_id, operation.change_id),
						eq(WorkspaceChangeDiffs.source_command_id, operation.message_id),
					),
				);

			if (projection.diff_state === "legacy_unavailable") {
				if (rows.length !== 0) {
					return yield* new JournalInvariantError({
						message: `Legacy workspace change ${operation.change_id} owns diff state`,
					});
				}

				return;
			}

			if (projection.diff_state !== "available" || rows.length !== 1) {
				return yield* new JournalInvariantError({
					message: `Workspace change ${operation.change_id} has invalid diff state`,
				});
			}

			const row = rows.at(0);
			if (row === undefined)
				return yield* new JournalInvariantError({
					message: `Workspace change ${operation.change_id} has no diff row`,
				});
			const [projection_before, projection_after, row_before, row_after] = yield* Effect.all([
				DecodeStoredIdentity(
					projection.before_identity_json,
					`Workspace change ${operation.change_id} has invalid before identity`,
				),
				DecodeStoredIdentity(
					projection.after_identity_json,
					`Workspace change ${operation.change_id} has invalid after identity`,
				),
				DecodeStoredIdentity(
					row.before_identity_json,
					`Workspace diff ${operation.change_id} has invalid before identity`,
				),
				DecodeStoredIdentity(
					row.after_identity_json,
					`Workspace diff ${operation.change_id} has invalid after identity`,
				),
			]);
			const patch = Uint8Array.from(row.patch);
			const digest = yield* crypto.digest("SHA-256", patch).pipe(
				Effect.mapError(
					() =>
						new JournalInvariantError({
							message: `Workspace diff ${operation.change_id} could not be verified`,
						}),
				),
			);
			const patch_hash = Encoding.encodeHex(digest);
			const patch_text = yield* Effect.try({
				catch: () =>
					new JournalInvariantError({
						message: `Workspace diff ${operation.change_id} is not UTF-8`,
					}),
				try: () => new TextDecoder("utf-8", { fatal: true }).decode(patch),
			});
			const rendered_line_count =
				patch_text.length === 0
					? 0
					: patch_text.split("\n").length - (patch_text.endsWith("\n") ? 1 : 0);

			if (
				row.change_id !== operation.change_id ||
				row.source_command_id !== operation.message_id ||
				row.thread_id !== operation.thread_id ||
				row.workspace_id !== operation.workspace_id ||
				row.path !== operation.path ||
				row.format !== "unified" ||
				row.format_version !== operation.diff_format_version ||
				row.format_version !== workspace_diff_format_version ||
				row.context_lines !== workspace_diff_context_lines ||
				row.patch_byte_count !== patch.byteLength ||
				row.patch_byte_count > workspace_diff_maximum_bytes ||
				row.patch_hash !== patch_hash ||
				row.added_line_count < 0 ||
				row.added_line_count > workspace_diff_maximum_lines_per_side ||
				row.removed_line_count < 0 ||
				row.removed_line_count > workspace_diff_maximum_lines_per_side ||
				rendered_line_count > workspace_diff_maximum_rendered_lines ||
				!workspace_diff_patch_matches_path(patch_text, operation.path) ||
				!identities_match(projection_before, operation.expected_identity) ||
				!identities_match(projection_before, row_before) ||
				!identities_match(projection_after, operation.result_identity) ||
				!identities_match(projection_after, row_after)
			) {
				return yield* new JournalInvariantError({
					message: `Workspace diff ${operation.change_id} is corrupt`,
				});
			}

			if (
				prepared_diff !== undefined &&
				(prepared_diff.change_id !== row.change_id ||
					prepared_diff.message_id !== row.source_command_id ||
					prepared_diff.thread_id !== row.thread_id ||
					prepared_diff.workspace_id !== row.workspace_id ||
					prepared_diff.path !== row.path ||
					prepared_diff.format !== row.format ||
					prepared_diff.format_version !== row.format_version ||
					prepared_diff.context_lines !== row.context_lines ||
					prepared_diff.added_line_count !== row.added_line_count ||
					prepared_diff.removed_line_count !== row.removed_line_count ||
					prepared_diff.patch_identity.byte_count !== row.patch_byte_count ||
					prepared_diff.patch_identity.content_hash !== row.patch_hash ||
					!bytes_match(prepared_diff.patch, patch))
			) {
				return yield* new WorkspaceChangeTransitionError({
					message: "Workspace prepared diff does not match the committed artifact",
				});
			}
		});

	const ValidateTransition = (
		transaction: typeof database.client,
		operation: WorkspaceChangeOperation,
	) =>
		Effect.gen(function* () {
			if (operation.action === "replace") {
				return undefined;
			}

			const stored = yield* ReadTransitionChange(transaction, operation);

			if (
				operation.action === "review" &&
				(stored.change.review_state !== "needs_review" ||
					stored.change.rollback_state !== "available")
			) {
				return yield* new WorkspaceChangeTransitionError({
					message: `Workspace change ${operation.change_id} cannot be reviewed`,
				});
			}

			if (
				operation.action === "rollback" &&
				((stored.change.review_state !== "needs_review" &&
					stored.change.review_state !== "reviewed") ||
					stored.change.rollback_state !== "available" ||
					!identities_match(stored.change.after_identity, operation.expected_identity))
			) {
				return yield* new WorkspaceChangeTransitionError({
					message: `Workspace change ${operation.change_id} cannot be rolled back`,
				});
			}

			return stored;
		});

	const ValidateRejectedState = (
		transaction: typeof database.client,
		operation: WorkspaceChangeOperation,
	) =>
		Effect.gen(function* () {
			const [command] = yield* transaction
				.select({ message_id: JournalCommands.message_id })
				.from(JournalCommands)
				.where(eq(JournalCommands.message_id, operation.message_id))
				.limit(1);
			const [event] = yield* transaction
				.select({ event_id: JournalEvents.event_id })
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.correlation_id, operation.message_id),
						ne(JournalEvents.event_type, "workspace.conflict.updated"),
					),
				)
				.limit(1);

			if (command || event) {
				return yield* new JournalInvariantError({
					message: `Rejected workspace operation ${operation.message_id} has journal state`,
				});
			}

			if (operation.action === "replace") {
				const [projection] = yield* transaction
					.select({ change_id: WorkspaceChanges.change_id })
					.from(WorkspaceChanges)
					.where(
						or(
							eq(WorkspaceChanges.change_id, operation.change_id),
							eq(WorkspaceChanges.source_command_id, operation.message_id),
						),
					)
					.limit(1);

				if (projection) {
					return yield* new JournalInvariantError({
						message: `Rejected workspace operation ${operation.message_id} has projection state`,
					});
				}

				return;
			}

			if (operation.action === "rollback") {
				yield* ValidateTransition(transaction, operation);

				return;
			}

			return yield* new JournalInvariantError({
				message: `Rejected workspace operation ${operation.message_id} has invalid action`,
			});
		});

	const HasAvailablePayload = (
		transaction: typeof database.client,
		operation: WorkspaceChangeOperation,
	) =>
		Effect.gen(function* () {
			const [payload] = yield* transaction
				.select({
					expected_byte_count: WorkspaceMutationPayloads.expected_byte_count,
					expected_hash: WorkspaceMutationPayloads.expected_hash,
					replacement_byte_count: WorkspaceMutationPayloads.replacement_byte_count,
					replacement_hash: WorkspaceMutationPayloads.replacement_hash,
					state: WorkspaceMutationPayloads.state,
					thread_id: WorkspaceMutationPayloads.thread_id,
				})
				.from(WorkspaceMutationPayloads)
				.where(eq(WorkspaceMutationPayloads.message_id, operation.message_id))
				.limit(1);

			if (!payload) {
				return false;
			}

			if (
				payload.state !== "available" ||
				payload.thread_id !== operation.thread_id ||
				typeof payload.expected_byte_count !== "number" ||
				typeof payload.expected_hash !== "string" ||
				typeof payload.replacement_byte_count !== "number" ||
				typeof payload.replacement_hash !== "string"
			) {
				return yield* new JournalInvariantError({
					message: `Workspace operation ${operation.message_id} has invalid staged payload state`,
				});
			}

			if (operation.action === "review") {
				return yield* new JournalInvariantError({
					message: `Workspace review ${operation.message_id} owns mutation payload state`,
				});
			}

			const expected_identity = {
				algorithm: "sha256" as const,
				byte_count: payload.expected_byte_count,
				content_hash: payload.expected_hash,
			};
			const replacement_identity = {
				algorithm: "sha256" as const,
				byte_count: payload.replacement_byte_count,
				content_hash: payload.replacement_hash,
			};
			let intended_replacement: ContentIdentityValue;

			if (operation.action === "replace") {
				intended_replacement = operation.result_identity;
			} else {
				const transition = yield* ValidateTransition(transaction, operation);

				if (!transition) {
					return yield* new JournalInvariantError({
						message: `Workspace rollback ${operation.message_id} has no source transition`,
					});
				}

				intended_replacement = transition.change.before_identity;
			}

			if (
				!identities_match(expected_identity, operation.expected_identity) ||
				!identities_match(replacement_identity, intended_replacement)
			) {
				return yield* new JournalInvariantError({
					message: `Workspace operation ${operation.message_id} has mismatched staged payload state`,
				});
			}

			return true;
		});

	const ReadOperation = (message_id: string) =>
		database.client
			.select()
			.from(WorkspaceChangeOperations)
			.where(eq(WorkspaceChangeOperations.message_id, message_id))
			.limit(1)
			.pipe(
				Effect.flatMap(([row]) =>
					row
						? DecodeOperation(row).pipe(Effect.map(Option.some))
						: Effect.succeed(Option.none<WorkspaceChangeOperation>()),
				),
				Effect.mapError(normalize_error),
			);

	const ReadChange = (change_id: string) =>
		database.client
			.select()
			.from(WorkspaceChanges)
			.where(eq(WorkspaceChanges.change_id, change_id))
			.limit(1)
			.pipe(
				Effect.flatMap(([row]) =>
					row
						? DecodeChange(row).pipe(Effect.map(Option.some))
						: Effect.succeed(Option.none<WorkspaceChangeValue>()),
				),
				Effect.mapError(normalize_error),
			);

	const ReadDuplicate = (
		transaction: typeof database.client,
		operation: WorkspaceChangeOperation,
		prepared_diff?: PreparedWorkspaceChangeDiff,
	) =>
		Effect.gen(function* () {
			const [command] = yield* transaction
				.select()
				.from(JournalCommands)
				.where(eq(JournalCommands.message_id, operation.message_id))
				.limit(1);

			if (!command) {
				return yield* new JournalInvariantError({
					message: `Workspace command ${operation.message_id} is invalid`,
				});
			}

			const command_raw_origin = yield* DecodeStoredRawOrigin(
				command.raw_origin_json,
				`Workspace command ${operation.message_id} has invalid attribution`,
			);
			const expected_agent_id = operation.action === "replace" ? operation.agent_id : null;
			const expected_raw_origin =
				operation.action === "replace" ? operation.raw_origin : undefined;
			const expected_run_id = operation.action === "replace" ? operation.run_id : null;

			if (
				command.agent_id !== expected_agent_id ||
				command.assigned_run_id !== null ||
				command.causation_id !== null ||
				command.origin !== "frontend" ||
				command.payload_type !== "workspace.change.command" ||
				!raw_origins_match(command_raw_origin, expected_raw_origin) ||
				command.run_id !== expected_run_id ||
				command.schema_version !== 1 ||
				command.sent_at !== operation.sent_at ||
				command.status !== "accepted" ||
				command.thread_id !== operation.thread_id
			) {
				return yield* new JournalInvariantError({
					message: `Workspace command ${operation.message_id} has invalid journal ownership`,
				});
			}

			const identity = yield* DecodeJson(command.payload_json).pipe(
				Effect.flatMap(
					Schema.decodeUnknownEffect(WorkspaceChangeCommandIdentity, {
						onExcessProperty: "error",
					}),
				),
				Effect.mapError(
					() =>
						new JournalInvariantError({
							message: `Workspace command ${operation.message_id} has invalid identity`,
						}),
				),
			);

			if (
				identity.action !== operation.action ||
				identity.change_id !== operation.change_id ||
				identity.request_fingerprint !== operation.request_fingerprint
			) {
				return yield* new JournalInvariantError({
					message: `Workspace command ${operation.message_id} identity does not match operation`,
				});
			}

			const rows = yield* transaction
				.select()
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.correlation_id, operation.message_id),
						eq(JournalEvents.event_type, "workspace.change.updated"),
					),
				)
				.orderBy(asc(JournalEvents.sequence));

			if (rows.length !== 1) {
				return yield* new JournalInvariantError({
					message: `Workspace operation ${operation.message_id} must have exactly one event`,
				});
			}

			const row = rows.at(0);
			if (row === undefined)
				return yield* new JournalInvariantError({
					message: `Workspace operation ${operation.message_id} has no event row`,
				});
			const event_raw_origin = yield* DecodeStoredRawOrigin(
				row.raw_origin_json,
				`Workspace event ${row.event_id} has invalid attribution`,
			);
			const expected_event_agent_id =
				operation.action === "replace" ? operation.agent_id : null;
			const expected_event_raw_origin =
				operation.action === "replace" ? operation.raw_origin : undefined;
			const expected_event_run_id = operation.action === "replace" ? operation.run_id : null;

			if (
				row.agent_id !== expected_event_agent_id ||
				row.causation_id !== operation.message_id ||
				row.event_type !== "workspace.change.updated" ||
				row.origin !== "backend" ||
				!raw_origins_match(event_raw_origin, expected_event_raw_origin) ||
				row.run_id !== expected_event_run_id ||
				row.schema_version !== 1 ||
				row.stream_id !== `thread:${operation.thread_id}` ||
				row.thread_id !== operation.thread_id
			) {
				return yield* new JournalInvariantError({
					message: `Workspace event ${row.event_id} has invalid journal ownership`,
				});
			}

			const event = yield* DecodeEvent(row);

			if (!event_matches_operation(event, operation)) {
				return yield* new JournalInvariantError({
					message: `Workspace event ${row.event_id} does not match its operation`,
				});
			}

			const projection = yield* ReadTransitionChange(transaction, operation);

			if (!change_identity_matches(event.payload.change, projection.change)) {
				return yield* new JournalInvariantError({
					message: `Workspace event ${row.event_id} does not match its projection`,
				});
			}

			if (operation.action === "replace") {
				yield* ValidateStoredReplaceDiff(
					transaction,
					operation,
					projection.row,
					prepared_diff,
				);
			}

			return event;
		});

	return {
		AppendJournalEventInTransaction,
		DecodeStoredIdentity,
		DecodeJson,
		EnsureLiveThread,
		HasAvailablePayload,
		ReadChange,
		ReadDuplicate,
		ReadOperation,
		ReadTransitionChange,
		ValidatePreparedDiff,
		ValidateRejectedState,
		ValidateStoredReplaceDiff,
		ValidateTransition,
		change_identity_matches,
		command_payload_json,
		database,
		event_matches_operation,
		immutable_operations_match,
		identities_match,
		metadata,
		normalize_commit_error,
		normalize_error,
		notifier,
		operation_from_claim,
	};
});

export class WorkspaceChangeContext extends Context.Service<
	WorkspaceChangeContext,
	Effect.Success<typeof MakeWorkspaceChangeContext>
>()("Artisan/WorkspaceChangeContext") {}
