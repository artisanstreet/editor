import { Context, Effect, Option, Ref } from "effect";

import {
	type CommandEnvelope,
	type EventEnvelope,
	type GitIndexStageRequestEnvelope,
	type GitIndexUnstageRequestEnvelope,
	type GitMutationResolveEnvelope,
	type ModelFavoriteUpdateEnvelope,
	type OutboundControlEnvelope,
	type ProtocolErrorDetail,
	type SessionDefaultsUpdateEnvelope,
	SnowflakeId,
	type ThreadCreateEnvelope,
	type ThreadRetentionUpdateEnvelope,
	type WorkspaceChangeReviewEnvelope,
	type WorkspaceChangeRollbackEnvelope,
	type WorkspaceFileReplaceEnvelope,
} from "@artisan/protocol";

import { GitService, GitServiceError } from "../../git/service";
import { CommandIdConflict, JournalStore } from "../../persistence/journal-store";
import { ThreadReadModel } from "../../persistence/thread-read-model";
import { RuntimeMetadata } from "../../runtime/metadata";
import { ProtocolRouter } from "../router";
import { model_favorites_thread_id } from "../../model-favorites/service";
import { session_defaults_thread_id } from "../../settings/session-defaults-service";
import { thread_retention_policy_thread_id } from "../../threads/thread-retention-policy";
import { WorkspaceFileService, WorkspaceFileServiceError } from "../../workspace/files/service";
import type { ConnectionState, ReadyState } from "../connection-state";

export class ReadyConnectionRuntime extends Context.Service<
	ReadyConnectionRuntime,
	{
		readonly DeliverLiveEvents: (
			events: ReadonlyArray<EventEnvelope>,
		) => Effect.Effect<void, unknown>;
		readonly Enqueue: (envelope: OutboundControlEnvelope) => Effect.Effect<void>;
		readonly EnqueueError: (
			current: ConnectionState,
			code: string,
			message: string,
			retryable: boolean,
			correlation_id?: string,
		) => Effect.Effect<void>;
		readonly state: Ref.Ref<ConnectionState>;
	}
>()("Artisan/ReadyConnectionRuntime") {}

const workspace_error_detail = (error: unknown): ProtocolErrorDetail =>
	error instanceof WorkspaceFileServiceError && error.reason === "changed"
		? {
				code: "workspace.conflict",
				message: "The workspace file changed before the requested mutation could apply.",
				retryable: false,
			}
		: {
				code: "workspace.unavailable",
				message: "The workspace operation could not be completed.",
				retryable: true,
			};

const git_error_detail = (error: unknown): ProtocolErrorDetail => {
	if (error instanceof CommandIdConflict)
		return {
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		};
	if (!(error instanceof GitServiceError))
		return {
			code: "git.unavailable",
			message: "The Git operation could not be completed.",
			retryable: true,
		};
	switch (error.reason) {
		case "busy":
			return {
				code: "git.busy",
				message: "Another Git mutation is already active for this workspace.",
				retryable: true,
			};
		case "changed":
			return {
				code: "git.changed",
				message: "The Git workspace changed; refresh before retrying.",
				retryable: false,
			};
		case "id_conflict":
			return {
				code: "command.id_conflict",
				message: "This command id has already been used for different Git intent.",
				retryable: false,
			};
		case "invalid_path":
			return {
				code: "git.invalid_path",
				message: "One or more paths are not eligible for this Git mutation.",
				retryable: false,
			};
		case "invariant":
			return {
				code: "git.invariant_failed",
				message: "The durable Git state failed validation.",
				retryable: false,
			};
		case "not_repository":
			return {
				code: "git.not_repository",
				message: "The workspace is not a Git repository.",
				retryable: false,
			};
		case "unsupported_state":
			return {
				code: "git.unsupported_state",
				message: "The Git workspace state does not support this mutation.",
				retryable: false,
			};
		case "unavailable":
			return {
				code: "git.unavailable",
				message: "The Git operation could not be completed.",
				retryable: error.retryable,
			};
	}
};

export const MakeReadyMutations = Effect.gen(function* () {
	const runtime = yield* ReadyConnectionRuntime;
	const router = yield* ProtocolRouter;
	const journal = yield* JournalStore;
	const threads = yield* ThreadReadModel;
	const metadata = yield* RuntimeMetadata;
	const snowflake_id = yield* SnowflakeId;
	const workspace_files = yield* WorkspaceFileService;
	const git = yield* GitService;

	const DeliverUndelivered = (events: ReadonlyArray<EventEnvelope>) =>
		Effect.gen(function* () {
			const current = yield* Ref.get(runtime.state);
			const undelivered =
				current._tag === "Ready"
					? events.filter(
							(event) => event.journal_sequence > current.delivered_journal_sequence,
						)
					: [];
			if (undelivered.length > 0) yield* runtime.DeliverLiveEvents(undelivered);
		});

	const HandleCommand = (command: CommandEnvelope) =>
		Effect.gen(function* () {
			if (command.payload.type === "thread.create") {
				const current = yield* Ref.get(runtime.state);
				return yield* runtime.EnqueueError(
					current,
					"protocol.legacy_thread_create",
					"Clients must create threads through thread.create.request so Forge owns the thread identity.",
					false,
					command.message_id,
				);
			}
			const output = yield* router.RouteCommand(command);
			const events = output.filter(
				(envelope): envelope is EventEnvelope => envelope.kind === "event",
			);
			yield* Effect.forEach(
				output.filter((envelope) => envelope.kind !== "event"),
				runtime.Enqueue,
				{ discard: true },
			);
			yield* DeliverUndelivered(events);
		});

	const HandleThreadCreate = (request: ThreadCreateEnvelope, current: ReadyState) =>
		Effect.gen(function* () {
			const existing = yield* journal.FindCommandThreadId(request.message_id);
			const thread_id = yield* Option.match(existing, {
				onNone: () => snowflake_id.MakeBare,
				onSome: Effect.succeed,
			});
			const output = yield* router.RouteCommand({
				kind: "command",
				message_id: request.message_id,
				origin: request.origin,
				payload: { ...request.payload, type: "thread.create" },
				protocol_version: request.protocol_version,
				schema_version: request.schema_version,
				sent_at: request.sent_at,
				thread_id,
			});
			const receipt = output.find((envelope) => envelope.kind === "command.receipt");
			if (receipt?.kind !== "command.receipt" || receipt.payload.status === "rejected") {
				const detail =
					receipt?.kind === "command.receipt" && receipt.payload.status === "rejected"
						? receipt.payload.error
						: {
								code: "thread.create_failed",
								message: "Forge could not durably create the thread.",
								retryable: true,
							};
				return yield* runtime.EnqueueError(
					current,
					detail.code,
					detail.message,
					detail.retryable,
					request.message_id,
				);
			}
			const projection = yield* Option.match(yield* threads.Lookup(thread_id), {
				onNone: () =>
					Effect.fail(
						new Error("Created thread is missing from its authoritative projection"),
					),
				onSome: Effect.succeed,
			});
			yield* runtime.Enqueue({
				correlation_id: request.message_id,
				kind: "thread.create.result",
				message_id: yield* metadata.MakeId("message"),
				origin: "backend",
				payload: projection,
				protocol_version: 1,
				schema_version: 1,
				sent_at: yield* metadata.Now,
			});
			yield* DeliverUndelivered(
				output.filter((envelope): envelope is EventEnvelope => envelope.kind === "event"),
			);
		}).pipe(
			Effect.catchCause(() =>
				runtime.EnqueueError(
					current,
					"thread.create_failed",
					"Forge could not durably create the thread.",
					true,
					request.message_id,
				),
			),
		);

	const HandleRetentionUpdate = (update: ThreadRetentionUpdateEnvelope) =>
		HandleCommand({
			kind: "command",
			message_id: update.message_id,
			origin: update.origin,
			payload: { ...update.payload, type: "thread.retention.update" },
			protocol_version: update.protocol_version,
			schema_version: update.schema_version,
			sent_at: update.sent_at,
			thread_id: thread_retention_policy_thread_id,
		});
	const HandleSessionDefaultsUpdate = (update: SessionDefaultsUpdateEnvelope) =>
		HandleCommand({
			kind: "command",
			message_id: update.message_id,
			origin: update.origin,
			payload: { ...update.payload, type: "session.defaults.update" },
			protocol_version: update.protocol_version,
			schema_version: update.schema_version,
			sent_at: update.sent_at,
			thread_id: session_defaults_thread_id,
		});
	const HandleModelFavoriteUpdate = (update: ModelFavoriteUpdateEnvelope) =>
		HandleCommand({
			kind: "command",
			message_id: update.message_id,
			origin: update.origin,
			payload: { ...update.payload, type: "model.favorite.update" },
			protocol_version: update.protocol_version,
			schema_version: update.schema_version,
			sent_at: update.sent_at,
			thread_id: model_favorites_thread_id,
		});

	type WorkspaceMutationEnvelope =
		| WorkspaceChangeReviewEnvelope
		| WorkspaceChangeRollbackEnvelope
		| WorkspaceFileReplaceEnvelope;
	const HandleWorkspaceMutation = (envelope: WorkspaceMutationEnvelope) => {
		const operation =
			envelope.kind === "workspace.file.replace"
				? workspace_files.Replace({
						...envelope.payload,
						agent_id: envelope.agent_id,
						message_id: envelope.message_id,
						raw_origin: envelope.raw_origin,
						run_id: envelope.run_id,
						sent_at: envelope.sent_at,
						thread_id: envelope.thread_id,
					})
				: envelope.kind === "workspace.change.review"
					? workspace_files.Review({
							...envelope.payload,
							message_id: envelope.message_id,
							sent_at: envelope.sent_at,
							thread_id: envelope.thread_id,
						})
					: workspace_files.Rollback({
							...envelope.payload,
							message_id: envelope.message_id,
							sent_at: envelope.sent_at,
							thread_id: envelope.thread_id,
						});
		return operation.pipe(
			Effect.matchEffect({
				onFailure: (error) => Receipt(envelope, undefined, workspace_error_detail(error)),
				onSuccess: (acceptance) => Receipt(envelope, acceptance),
			}),
		);
	};

	const Receipt = (
		envelope: WorkspaceMutationEnvelope | GitMutationEnvelope,
		acceptance?: {
			readonly event: { readonly journal_sequence: number };
			readonly status: "accepted" | "duplicate";
		},
		error?: ProtocolErrorDetail,
	) =>
		Effect.gen(function* () {
			yield* runtime.Enqueue({
				...(!("agent_id" in envelope) || envelope.agent_id === undefined
					? {}
					: { agent_id: envelope.agent_id }),
				causation_id: envelope.message_id,
				correlation_id: envelope.message_id,
				kind: "command.receipt",
				message_id: yield* metadata.MakeId("message"),
				origin: "backend",
				payload:
					error === undefined && acceptance !== undefined
						? {
								journal_sequence: acceptance.event.journal_sequence,
								status: acceptance.status,
							}
						: {
								error: error ?? workspace_error_detail(undefined),
								status: "rejected",
							},
				protocol_version: 1,
				...(!("run_id" in envelope) || envelope.run_id === undefined
					? {}
					: { run_id: envelope.run_id }),
				schema_version: 1,
				sent_at: yield* metadata.Now,
				thread_id: envelope.thread_id,
			});
		});

	type GitMutationEnvelope =
		| GitIndexStageRequestEnvelope
		| GitIndexUnstageRequestEnvelope
		| GitMutationResolveEnvelope;
	const HandleGitMutation = (envelope: GitMutationEnvelope) =>
		(envelope.kind === "git.mutation.resolve"
			? git.Resolve(envelope)
			: git.Request(envelope)
		).pipe(
			Effect.matchEffect({
				onFailure: (error) => Receipt(envelope, undefined, git_error_detail(error)),
				onSuccess: (acceptance) =>
					Receipt(envelope, acceptance).pipe(
						Effect.andThen(
							Effect.gen(function* () {
								const current = yield* Ref.get(runtime.state);
								if (current._tag !== "Ready") return;
								yield* journal
									.ReadReplay({
										after_journal_sequence: current.delivered_journal_sequence,
									})
									.pipe(
										Effect.flatMap(runtime.DeliverLiveEvents),
										Effect.catch(() =>
											runtime.EnqueueError(
												current,
												"journal.replay_failed",
												"Live journal delivery could not be resumed.",
												true,
												envelope.message_id,
											),
										),
									);
							}),
						),
					),
			}),
		);

	return {
		HandleCommand,
		HandleGitMutation,
		HandleModelFavoriteUpdate,
		HandleSessionDefaultsUpdate,
		HandleThreadCreate,
		HandleRetentionUpdate,
		HandleWorkspaceMutation,
	};
});
