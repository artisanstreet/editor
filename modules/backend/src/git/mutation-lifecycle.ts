import { and, eq, or } from "drizzle-orm";
import { Context, Effect, Schema } from "effect";

import {
	GitMutationResolveEnvelope,
	GitMutationUpdatedEvent,
	GitWorkspaceUpdatedEvent,
	Identifier,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { GitMutationOperations } from "../persistence/tables";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import { RuntimeMetadata } from "../runtime/metadata";
import {
	GitMutationRequestEnvelope,
	GitMutationSucceededInput,
	GitMutationTerminalInput,
	GitRepositoryConflict,
	StoredGitMutationRow,
	git_dispatch_lease_milliseconds,
	type MutationIdentity,
} from "./contracts";
import { GitRuntime } from "./runtime";
export const MakeGitMutationLifecycle = Effect.gen(function* () {
	const database = yield* Database;
	const metadata = yield* RuntimeMetadata;
	const notifier = yield* JournalNotifier;
	const {
		AppendEvent,
		ComputeFingerprint,
		Decode,
		DecodeMutationRow,
		EnsureLiveThread,
		MakeMutationProjection,
		MakeWorkspaceProjection,
		ReadEventBySequence,
		ReadFirstEvent,
		ReadLastEvent,
		ReadMutationTransaction,
		ReadWorkspaceTransaction,
		WriteWorkspaceProjection,
		canonical_paths,
		invariant,
		normalize_error,
		traces_equal,
	} = yield* GitRuntime;
	const RequestIdentity = (envelope: GitMutationRequestEnvelope): MutationIdentity => ({
		...(envelope.agent_id === undefined ? {} : { agent_id: envelope.agent_id }),
		approval_id: envelope.payload.approval_id,
		expected_snapshot_id: envelope.payload.expected_snapshot_id,
		expected_workspace_version: envelope.payload.expected_workspace_version,
		kind: envelope.kind === "git.index.stage.request" ? "stage" : "unstage",
		mutation_id: envelope.payload.mutation_id,
		paths: canonical_paths(envelope.payload.paths),
		...(envelope.raw_origin === undefined ? {} : { raw_origin: envelope.raw_origin }),
		request_message_id: envelope.message_id,
		requested_at: envelope.sent_at,
		...(envelope.run_id === undefined ? {} : { run_id: envelope.run_id }),
		thread_id: envelope.thread_id,
		workspace_id: envelope.payload.workspace_id,
	});

	const RequestMutation = (input: GitMutationRequestEnvelope) =>
		Decode(GitMutationRequestEnvelope, input, "request_mutation").pipe(
			Effect.flatMap((envelope) =>
				Effect.gen(function* () {
					const identity = RequestIdentity(envelope);
					const request_fingerprint = yield* ComputeFingerprint(identity);

					return yield* RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								yield* EnsureLiveThread(transaction, envelope.thread_id);
								const collisions = yield* transaction
									.select()
									.from(GitMutationOperations)
									.where(
										or(
											eq(
												GitMutationOperations.mutation_id,
												identity.mutation_id,
											),
											eq(
												GitMutationOperations.approval_id,
												identity.approval_id,
											),
											eq(
												GitMutationOperations.source_message_id,
												identity.request_message_id,
											),
											eq(
												GitMutationOperations.decision_message_id,
												identity.request_message_id,
											),
										),
									);

								if (collisions.length > 0) {
									if (collisions.length !== 1) {
										return yield* Effect.fail(
											invariant(
												"Git mutation identities resolve to multiple rows",
											),
										);
									}

									const existing = yield* DecodeMutationRow(collisions[0]);

									if (
										existing.request_fingerprint !== request_fingerprint ||
										existing.identity.mutation_id !== identity.mutation_id ||
										existing.identity.approval_id !== identity.approval_id ||
										existing.identity.request_message_id !==
											identity.request_message_id
									) {
										return yield* Effect.fail(
											new GitRepositoryConflict({
												reason: "mutation_conflict",
											}),
										);
									}

									return {
										event: yield* ReadFirstEvent(
											transaction,
											identity.request_message_id,
											"git.mutation.updated",
										),
										mutation: existing.projection,
										status: "duplicate" as const,
									};
								}

								const workspace = yield* ReadWorkspaceTransaction(
									transaction,
									identity.workspace_id,
								);

								if (
									workspace.snapshot_id !== identity.expected_snapshot_id ||
									workspace.version !== identity.expected_workspace_version
								) {
									return yield* Effect.fail(
										new GitRepositoryConflict({
											reason: "workspace_changed",
										}),
									);
								}

								const updated_at = yield* metadata.Now;
								const row = yield* Schema.decodeUnknownEffect(
									StoredGitMutationRow,
									{
										onExcessProperty: "error",
									},
								)({
									agent_id: identity.agent_id ?? null,
									approval_id: identity.approval_id,
									completed_at: null,
									decision_at: null,
									decision_message_id: null,
									dispatched_at: null,
									dispatch_lease_expires_at: null,
									dispatch_owner_id: null,
									expected_snapshot_id: identity.expected_snapshot_id,
									expected_workspace_version: identity.expected_workspace_version,
									failure_code: null,
									journal_sequence: null,
									kind: identity.kind,
									lifecycle: "awaiting_approval",
									mutation_id: identity.mutation_id,
									paths_json: JSON.stringify(identity.paths),
									raw_origin_json:
										identity.raw_origin === undefined
											? null
											: JSON.stringify(identity.raw_origin),
									request_fingerprint,
									requested_at: identity.requested_at,
									result_snapshot_id: null,
									result_workspace_version: null,
									run_id: identity.run_id ?? null,
									source_message_id: identity.request_message_id,
									thread_id: identity.thread_id,
									updated_at,
									workspace_id: identity.workspace_id,
								}).pipe(
									Effect.mapError(() =>
										invariant("New Git mutation row is invalid"),
									),
								);

								yield* transaction.insert(GitMutationOperations).values(row);
								const event = yield* AppendEvent(transaction, {
									...(identity.agent_id === undefined
										? {}
										: { agent_id: identity.agent_id }),
									causation_id: identity.request_message_id,
									correlation_id: identity.request_message_id,
									occurred_at: updated_at,
									payload_at: (journal_sequence) =>
										MakeMutationProjection(
											row,
											identity.paths,
											identity.raw_origin,
											undefined,
											journal_sequence,
										).pipe(
											Effect.flatMap((mutation) =>
												Schema.decodeUnknownEffect(
													GitMutationUpdatedEvent,
													{
														onExcessProperty: "error",
													},
												)({ mutation, type: "git.mutation.updated" }),
											),
											Effect.mapError(() =>
												invariant("Git mutation event is invalid"),
											),
										),
									...(identity.raw_origin === undefined
										? {}
										: { raw_origin: identity.raw_origin }),
									...(identity.run_id === undefined
										? {}
										: { run_id: identity.run_id }),
									thread_id: identity.thread_id,
								});
								yield* transaction
									.update(GitMutationOperations)
									.set({ journal_sequence: event.journal_sequence })
									.where(
										eq(GitMutationOperations.mutation_id, identity.mutation_id),
									);
								const mutation = yield* ReadMutationTransaction(
									transaction,
									identity.mutation_id,
								);

								return {
									event,
									mutation: mutation.projection,
									status: "accepted" as const,
								};
							}),
						),
					);
				}),
			),
			Effect.mapError(normalize_error),
			Effect.tap((result) =>
				result.status === "accepted"
					? notifier.Publish(result.event.journal_sequence)
					: Effect.void,
			),
		);

	const ResolveMutation = (input: typeof GitMutationResolveEnvelope.Type) =>
		Decode(GitMutationResolveEnvelope, input, "resolve_mutation").pipe(
			Effect.flatMap((envelope) =>
				RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* EnsureLiveThread(transaction, envelope.thread_id);
							const [message_collision] = yield* transaction
								.select()
								.from(GitMutationOperations)
								.where(
									or(
										eq(
											GitMutationOperations.source_message_id,
											envelope.message_id,
										),
										eq(
											GitMutationOperations.decision_message_id,
											envelope.message_id,
										),
									),
								)
								.limit(1);
							const mutation = yield* ReadMutationTransaction(
								transaction,
								envelope.payload.mutation_id,
							);
							const decision_trace = {
								...(envelope.agent_id === undefined
									? {}
									: { agent_id: envelope.agent_id }),
								...(envelope.raw_origin === undefined
									? {}
									: { raw_origin: envelope.raw_origin }),
								...(envelope.run_id === undefined
									? {}
									: { run_id: envelope.run_id }),
								thread_id: envelope.thread_id,
							};

							if (
								envelope.payload.approval_id !== mutation.identity.approval_id ||
								!traces_equal(decision_trace, mutation.identity)
							) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "decision_conflict" }),
								);
							}

							if (mutation.row.decision_message_id !== null) {
								const prior_approved = mutation.row.lifecycle !== "denied";

								if (
									mutation.row.decision_message_id !== envelope.message_id ||
									mutation.row.decision_at !== envelope.sent_at ||
									prior_approved !== envelope.payload.approved
								) {
									return yield* Effect.fail(
										new GitRepositoryConflict({
											reason: "decision_conflict",
										}),
									);
								}

								return {
									event: yield* ReadEventBySequence(
										transaction,
										mutation.projection.journal_sequence,
									),
									mutation: mutation.projection,
									status: "duplicate" as const,
								};
							}

							if (
								mutation.row.lifecycle !== "awaiting_approval" ||
								(message_collision !== undefined &&
									message_collision.mutation_id !== mutation.row.mutation_id)
							) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "decision_conflict" }),
								);
							}

							const updated_at = yield* metadata.Now;
							const lifecycle = envelope.payload.approved ? "approved" : "denied";
							const [updated] = yield* transaction
								.update(GitMutationOperations)
								.set({
									completed_at: envelope.payload.approved ? null : updated_at,
									decision_at: envelope.sent_at,
									decision_message_id: envelope.message_id,
									lifecycle,
									updated_at,
								})
								.where(
									and(
										eq(
											GitMutationOperations.mutation_id,
											mutation.row.mutation_id,
										),
										eq(GitMutationOperations.lifecycle, "awaiting_approval"),
									),
								)
								.returning();

							if (updated === undefined) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "decision_conflict" }),
								);
							}

							const event = yield* AppendEvent(transaction, {
								...(envelope.agent_id === undefined
									? {}
									: { agent_id: envelope.agent_id }),
								causation_id: envelope.message_id,
								correlation_id: envelope.message_id,
								occurred_at: updated_at,
								payload_at: (journal_sequence) =>
									DecodeMutationRow({ ...updated, journal_sequence }, true).pipe(
										Effect.flatMap(({ projection }) =>
											Schema.decodeUnknownEffect(GitMutationUpdatedEvent, {
												onExcessProperty: "error",
											})({
												mutation: projection,
												type: "git.mutation.updated",
											}),
										),
										Effect.mapError((cause) =>
											invariant(
												`Git decision event is invalid: ${String(cause)}`,
											),
										),
									),
								...(envelope.raw_origin === undefined
									? {}
									: { raw_origin: envelope.raw_origin }),
								...(envelope.run_id === undefined
									? {}
									: { run_id: envelope.run_id }),
								thread_id: envelope.thread_id,
							});
							yield* transaction
								.update(GitMutationOperations)
								.set({ journal_sequence: event.journal_sequence })
								.where(
									eq(GitMutationOperations.mutation_id, mutation.row.mutation_id),
								);
							const committed = yield* ReadMutationTransaction(
								transaction,
								mutation.row.mutation_id,
							);

							return {
								event,
								mutation: committed.projection,
								status: "accepted" as const,
							};
						}),
					),
				),
			),
			Effect.mapError(normalize_error),
			Effect.tap((result) =>
				result.status === "accepted"
					? notifier.Publish(result.event.journal_sequence)
					: Effect.void,
			),
		);

	const ClaimApproved = (input: string) =>
		Decode(Identifier, input, "claim_approved").pipe(
			Effect.flatMap((mutation_id) =>
				RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const mutation = yield* ReadMutationTransaction(
								transaction,
								mutation_id,
							);

							if (mutation.row.lifecycle !== "approved") {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "dispatch_conflict" }),
								);
							}
							if (mutation.row.decision_message_id === null) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "dispatch_conflict" }),
								);
							}

							yield* EnsureLiveThread(transaction, mutation.row.thread_id);
							const [busy] = yield* transaction
								.select({ mutation_id: GitMutationOperations.mutation_id })
								.from(GitMutationOperations)
								.where(
									and(
										eq(
											GitMutationOperations.workspace_id,
											mutation.row.workspace_id,
										),
										eq(GitMutationOperations.lifecycle, "dispatching"),
									),
								)
								.limit(1);

							if (busy !== undefined) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "workspace_busy" }),
								);
							}

							const updated_at = yield* metadata.Now;
							const dispatch_lease_expires_at = new Date(
								Date.parse(updated_at) + git_dispatch_lease_milliseconds,
							).toISOString();
							const [updated] = yield* transaction
								.update(GitMutationOperations)
								.set({
									dispatched_at: updated_at,
									dispatch_lease_expires_at,
									dispatch_owner_id: metadata.instance_id,
									lifecycle: "dispatching",
									updated_at,
								})
								.where(
									and(
										eq(GitMutationOperations.mutation_id, mutation_id),
										eq(GitMutationOperations.lifecycle, "approved"),
									),
								)
								.returning();

							if (updated === undefined) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "dispatch_conflict" }),
								);
							}

							const correlation_id = `git_mutation:${mutation_id}:dispatch`;
							const event = yield* AppendEvent(transaction, {
								...(mutation.identity.agent_id === undefined
									? {}
									: { agent_id: mutation.identity.agent_id }),
								causation_id: mutation.row.decision_message_id,
								correlation_id,
								occurred_at: updated_at,
								payload_at: (journal_sequence) =>
									DecodeMutationRow({ ...updated, journal_sequence }, true).pipe(
										Effect.flatMap(({ projection }) =>
											Schema.decodeUnknownEffect(GitMutationUpdatedEvent, {
												onExcessProperty: "error",
											})({
												mutation: projection,
												type: "git.mutation.updated",
											}),
										),
										Effect.mapError(() =>
											invariant("Git dispatch event is invalid"),
										),
									),
								...(mutation.identity.raw_origin === undefined
									? {}
									: { raw_origin: mutation.identity.raw_origin }),
								...(mutation.identity.run_id === undefined
									? {}
									: { run_id: mutation.identity.run_id }),
								thread_id: mutation.row.thread_id,
							});
							yield* transaction
								.update(GitMutationOperations)
								.set({ journal_sequence: event.journal_sequence })
								.where(eq(GitMutationOperations.mutation_id, mutation_id));
							const committed = yield* ReadMutationTransaction(
								transaction,
								mutation_id,
							);

							return {
								event,
								mutation: committed.projection,
								status: "accepted" as const,
							};
						}),
					),
				),
			),
			Effect.mapError(normalize_error),
			Effect.tap((result) => notifier.Publish(result.event.journal_sequence)),
		);

	const CommitTerminal = (input: GitMutationTerminalInput) =>
		Decode(GitMutationTerminalInput, input, "commit_terminal").pipe(
			Effect.flatMap((terminal) =>
				RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const mutation = yield* ReadMutationTransaction(
								transaction,
								terminal.mutation_id,
							);

							if (mutation.row.lifecycle === terminal.state) {
								if (
									JSON.stringify(mutation.projection.failure) !==
									JSON.stringify(terminal.failure)
								) {
									return yield* Effect.fail(
										new GitRepositoryConflict({
											reason: "terminal_conflict",
										}),
									);
								}

								return {
									event: yield* ReadEventBySequence(
										transaction,
										mutation.projection.journal_sequence,
									),
									mutation: mutation.projection,
									status: "duplicate" as const,
								};
							}

							if (mutation.row.lifecycle !== "dispatching") {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "terminal_conflict" }),
								);
							}

							const updated_at = yield* metadata.Now;
							const [updated] = yield* transaction
								.update(GitMutationOperations)
								.set({
									completed_at: updated_at,
									failure_code: JSON.stringify(terminal.failure),
									lifecycle: terminal.state,
									updated_at,
								})
								.where(
									and(
										eq(GitMutationOperations.mutation_id, terminal.mutation_id),
										eq(GitMutationOperations.lifecycle, "dispatching"),
									),
								)
								.returning();

							if (updated === undefined) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "terminal_conflict" }),
								);
							}

							const event = yield* AppendEvent(transaction, {
								...(mutation.identity.agent_id === undefined
									? {}
									: { agent_id: mutation.identity.agent_id }),
								causation_id: terminal.mutation_id,
								correlation_id: mutation.identity.request_message_id,
								occurred_at: updated_at,
								payload_at: (journal_sequence) =>
									DecodeMutationRow({ ...updated, journal_sequence }, true).pipe(
										Effect.flatMap(({ projection }) =>
											Schema.decodeUnknownEffect(GitMutationUpdatedEvent, {
												onExcessProperty: "error",
											})({
												mutation: projection,
												type: "git.mutation.updated",
											}),
										),
										Effect.mapError(() =>
											invariant("Terminal Git event is invalid"),
										),
									),
								...(mutation.identity.raw_origin === undefined
									? {}
									: { raw_origin: mutation.identity.raw_origin }),
								...(mutation.identity.run_id === undefined
									? {}
									: { run_id: mutation.identity.run_id }),
								thread_id: mutation.identity.thread_id,
							});
							yield* transaction
								.update(GitMutationOperations)
								.set({ journal_sequence: event.journal_sequence })
								.where(eq(GitMutationOperations.mutation_id, terminal.mutation_id));
							const committed = yield* ReadMutationTransaction(
								transaction,
								terminal.mutation_id,
							);

							return {
								event,
								mutation: committed.projection,
								status: "accepted" as const,
							};
						}),
					),
				),
			),
			Effect.mapError(normalize_error),
			Effect.tap((result) =>
				result.status === "accepted"
					? notifier.Publish(result.event.journal_sequence)
					: Effect.void,
			),
		);

	const CommitSucceeded = (input: GitMutationSucceededInput) =>
		Decode(GitMutationSucceededInput, input, "commit_succeeded").pipe(
			Effect.flatMap((success) =>
				RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const mutation = yield* ReadMutationTransaction(
								transaction,
								success.mutation_id,
							);

							if (mutation.row.lifecycle === "succeeded") {
								const workspace = yield* ReadWorkspaceTransaction(
									transaction,
									mutation.row.workspace_id,
								);
								const candidate = yield* MakeWorkspaceProjection(
									success.workspace,
									workspace.version,
									workspace.journal_sequence,
								);

								if (
									mutation.result_snapshot_id !== success.workspace.snapshot_id ||
									mutation.row.result_workspace_version !== workspace.version ||
									JSON.stringify(candidate) !== JSON.stringify(workspace)
								) {
									return yield* Effect.fail(
										new GitRepositoryConflict({
											reason: "terminal_conflict",
										}),
									);
								}

								return {
									mutation: mutation.projection,
									mutation_event: yield* ReadEventBySequence(
										transaction,
										mutation.projection.journal_sequence,
									),
									status: "duplicate" as const,
									workspace,
									workspace_event: yield* ReadLastEvent(
										transaction,
										mutation.identity.request_message_id,
										"git.workspace.updated",
									),
								};
							}

							if (
								mutation.row.lifecycle !== "dispatching" ||
								success.workspace.workspace_id !== mutation.row.workspace_id
							) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "terminal_conflict" }),
								);
							}

							const current = yield* ReadWorkspaceTransaction(
								transaction,
								mutation.row.workspace_id,
							);

							if (
								current.snapshot_id !== mutation.row.expected_snapshot_id ||
								current.version !== mutation.row.expected_workspace_version
							) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "workspace_changed" }),
								);
							}

							const updated_at = yield* metadata.Now;
							const next_version = current.version + 1;
							const workspace_event = yield* AppendEvent(transaction, {
								...(mutation.identity.agent_id === undefined
									? {}
									: { agent_id: mutation.identity.agent_id }),
								causation_id: mutation.identity.mutation_id,
								correlation_id: mutation.identity.request_message_id,
								occurred_at: updated_at,
								payload_at: (journal_sequence) =>
									MakeWorkspaceProjection(
										success.workspace,
										next_version,
										journal_sequence,
									).pipe(
										Effect.flatMap((workspace) =>
											Schema.decodeUnknownEffect(GitWorkspaceUpdatedEvent, {
												onExcessProperty: "error",
											})({
												cause: "mutation",
												type: "git.workspace.updated",
												workspace,
											}),
										),
										Effect.mapError(() =>
											invariant("Succeeded Git workspace event is invalid"),
										),
									),
								...(mutation.identity.raw_origin === undefined
									? {}
									: { raw_origin: mutation.identity.raw_origin }),
								...(mutation.identity.run_id === undefined
									? {}
									: { run_id: mutation.identity.run_id }),
								thread_id: mutation.identity.thread_id,
							});
							const workspace = yield* MakeWorkspaceProjection(
								success.workspace,
								next_version,
								workspace_event.journal_sequence,
							);
							yield* WriteWorkspaceProjection(
								transaction,
								workspace,
								updated_at,
								current,
							);
							const [updated] = yield* transaction
								.update(GitMutationOperations)
								.set({
									completed_at: updated_at,
									lifecycle: "succeeded",
									result_snapshot_id: workspace.snapshot_id,
									result_workspace_version: workspace.version,
									updated_at,
								})
								.where(
									and(
										eq(GitMutationOperations.mutation_id, success.mutation_id),
										eq(GitMutationOperations.lifecycle, "dispatching"),
									),
								)
								.returning();

							if (updated === undefined) {
								return yield* Effect.fail(
									new GitRepositoryConflict({ reason: "terminal_conflict" }),
								);
							}

							const mutation_event = yield* AppendEvent(transaction, {
								...(mutation.identity.agent_id === undefined
									? {}
									: { agent_id: mutation.identity.agent_id }),
								causation_id: mutation.identity.mutation_id,
								correlation_id: mutation.identity.request_message_id,
								occurred_at: updated_at,
								payload_at: (journal_sequence) =>
									DecodeMutationRow({ ...updated, journal_sequence }, true).pipe(
										Effect.flatMap(({ projection }) =>
											Schema.decodeUnknownEffect(GitMutationUpdatedEvent, {
												onExcessProperty: "error",
											})({
												mutation: projection,
												type: "git.mutation.updated",
											}),
										),
										Effect.mapError(() =>
											invariant("Succeeded Git mutation event is invalid"),
										),
									),
								...(mutation.identity.raw_origin === undefined
									? {}
									: { raw_origin: mutation.identity.raw_origin }),
								...(mutation.identity.run_id === undefined
									? {}
									: { run_id: mutation.identity.run_id }),
								thread_id: mutation.identity.thread_id,
							});
							yield* transaction
								.update(GitMutationOperations)
								.set({ journal_sequence: mutation_event.journal_sequence })
								.where(eq(GitMutationOperations.mutation_id, success.mutation_id));
							const committed = yield* ReadMutationTransaction(
								transaction,
								success.mutation_id,
							);

							return {
								mutation: committed.projection,
								mutation_event,
								status: "accepted" as const,
								workspace,
								workspace_event,
							};
						}),
					),
				),
			),
			Effect.mapError(normalize_error),
			Effect.tap((result) =>
				result.status === "accepted"
					? notifier.Publish(result.mutation_event.journal_sequence)
					: Effect.void,
			),
		);

	return {
		ClaimApproved,
		CommitSucceeded,
		CommitTerminal,
		RequestMutation,
		ResolveMutation,
	};
});

export class GitMutationLifecycle extends Context.Service<
	GitMutationLifecycle,
	Effect.Success<typeof MakeGitMutationLifecycle>
>()("Artisan/GitMutationLifecycle") {}
