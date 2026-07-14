import { Context, Crypto, Effect, Encoding, Layer, Option, Schema } from "effect";

import {
	Identifier,
	IsoDateTime,
	WorkspaceGitSessionQuery,
	type WorkspaceGitSessionQueryResult,
} from "@artisan/protocol";

import {
	WorkspaceGitSessionRepository,
	type WorkspaceGitSessionAcceptance,
	type WorkspaceGitSessionRepositoryError,
} from "./workspace-git-session-repository";
import {
	WorkspaceGitObservationError,
	WorkspaceGitObserver,
	type WorkspaceGitObservation,
} from "./workspace-git-observer";
import {
	WorkspaceEvidenceRecorder,
	type WorkspaceEvidenceRecorderError,
} from "../workspace/workspace-evidence-recorder";

const WorkspaceGitSessionRefresh = Schema.Struct({
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const WorkspaceGitProjection = Schema.Struct({
	kind: Schema.Literals(["checkout", "recovery", "mutation"]),
	operation_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

/** Supplies one frontend refresh command for a registered workspace. */
export type WorkspaceGitSessionRefresh = typeof WorkspaceGitSessionRefresh.Type;

/** Supplies one backend-owned observation after checkout or recovery. */
export type WorkspaceGitProjection = typeof WorkspaceGitProjection.Type;

/** Represents failures surfaced while observing, projecting, or recording Git evidence. */
export type WorkspaceGitSessionServiceError =
	| WorkspaceEvidenceRecorderError
	| WorkspaceGitObservationError
	| WorkspaceGitSessionRepositoryError;

/** Coordinates read-only Git observation with its durable session and evidence records. */
export class WorkspaceGitSessionService extends Context.Service<
	WorkspaceGitSessionService,
	{
		readonly Project: (
			input: WorkspaceGitProjection,
		) => Effect.Effect<WorkspaceGitSessionAcceptance, WorkspaceGitSessionServiceError>;
		readonly ProjectObserved: (
			input: WorkspaceGitProjection,
			observation: WorkspaceGitObservation,
		) => Effect.Effect<WorkspaceGitSessionAcceptance, WorkspaceGitSessionServiceError>;
		readonly Query: (
			query: typeof WorkspaceGitSessionQuery.Type,
		) => Effect.Effect<WorkspaceGitSessionQueryResult, WorkspaceGitSessionRepositoryError>;
		readonly RecoverEvidence: Effect.Effect<void, WorkspaceGitSessionServiceError>;
		readonly Refresh: (
			input: WorkspaceGitSessionRefresh,
		) => Effect.Effect<WorkspaceGitSessionAcceptance, WorkspaceGitSessionServiceError>;
	}
>()("Artisan/WorkspaceGitSessionService") {}

function projection_fingerprint(input: {
	readonly kind: "checkout" | "mutation" | "recovery" | "refresh";
	readonly operation_id: string;
	readonly sent_at: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}) {
	return JSON.stringify({
		kind: input.kind,
		operation_id: input.operation_id,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
		workspace_id: input.workspace_id,
	});
}

function public_session(observation: WorkspaceGitObservation) {
	return {
		blockers: observation.blockers,
		...(observation.branch === undefined ? {} : { branch: observation.branch }),
		changed_files: observation.changed_files,
		diff_stats: observation.diff_stats,
		has_diff: observation.has_diff,
		...(observation.head === undefined ? {} : { head: observation.head }),
		state: observation.state,
	};
}

/** Supplies the durable observation workflow over Git, SQLite, and workspace evidence. */
export const WorkspaceGitSessionServiceLive = Layer.effect(
	WorkspaceGitSessionService,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const evidence = yield* WorkspaceEvidenceRecorder;
		const observer = yield* WorkspaceGitObserver;
		const repository = yield* WorkspaceGitSessionRepository;

		const Fingerprint = (input: Parameters<typeof projection_fingerprint>[0]) =>
			crypto.digest("SHA-256", new TextEncoder().encode(projection_fingerprint(input))).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(
					(cause) => new WorkspaceGitObservationError({ cause, reason: "invalid_state" }),
				),
			);
		const RecordPendingEvidence = (operation_id?: string) =>
			Effect.gen(function* () {
				const pending = yield* repository.ListPendingEvidence;
				const selected =
					operation_id === undefined
						? pending
						: pending.filter((entry) => entry.operation_id === operation_id);

				yield* Effect.forEach(
					selected,
					(entry) =>
						Effect.gen(function* () {
							yield* evidence.RecordGitWorkspaceObserved(entry);
							yield* repository.MarkEvidenceRecorded(entry.operation_id);
						}),
					{ discard: true },
				);
			});
		type ProjectionInput = {
			readonly kind: "checkout" | "mutation" | "recovery" | "refresh";
			readonly operation_id: string;
			readonly sent_at: string;
			readonly source_command: boolean;
			readonly thread_id: string;
			readonly workspace_id: string;
		};
		type BackendProjectionInput = Omit<ProjectionInput, "kind" | "source_command"> & {
			readonly kind: "checkout" | "mutation" | "recovery";
			readonly source_command: false;
		};
		const CommitObservation = (
			input: ProjectionInput,
			observation: WorkspaceGitObservation,
			prepared_fingerprint?: string,
		) =>
			Effect.gen(function* () {
				const request_fingerprint =
					prepared_fingerprint === undefined
						? yield* Fingerprint(input)
						: prepared_fingerprint;
				const acceptance = yield* repository.Project({
					kind: input.kind,
					observed_at: observation.observed_at,
					operation_id: input.operation_id,
					...(observation.repository_root === undefined
						? {}
						: { repository_root: observation.repository_root }),
					request_fingerprint,
					...(observation.selected_worktree_path === undefined
						? {}
						: { selected_worktree_path: observation.selected_worktree_path }),
					session: public_session(observation),
					...(input.source_command
						? {
								source_command: {
									message_id: input.operation_id,
									sent_at: input.sent_at,
								},
							}
						: {}),
					thread_id: input.thread_id,
					workspace_id: input.workspace_id,
					worktrees: observation.adapter_worktrees.map(
						({ adapter_path, ...worktree }) => ({
							adapter_path,
							worktree,
						}),
					),
				});

				yield* RecordPendingEvidence(input.operation_id);

				return acceptance;
			});
		const ObserveAndProject = (input: ProjectionInput) =>
			Effect.gen(function* () {
				const observation = yield* observer.Observe(input.workspace_id);

				return yield* CommitObservation(input, observation);
			});
		const ReplayOrObserveAndProject = (input: BackendProjectionInput) =>
			Effect.gen(function* () {
				const request_fingerprint = yield* Fingerprint(input);
				const replay_input = {
					kind: input.kind,
					operation_id: input.operation_id,
					request_fingerprint,
					thread_id: input.thread_id,
					workspace_id: input.workspace_id,
				};
				const CompleteReplay = (acceptance: WorkspaceGitSessionAcceptance) =>
					RecordPendingEvidence(input.operation_id).pipe(Effect.as(acceptance));
				const replay = yield* repository.Replay(replay_input);

				if (Option.isSome(replay)) {
					return yield* CompleteReplay(replay.value);
				}

				const observation = yield* observer.Observe(input.workspace_id);

				return yield* CommitObservation(input, observation, request_fingerprint).pipe(
					Effect.catch((failure) =>
						repository
							.Replay(replay_input)
							.pipe(
								Effect.flatMap((committed) =>
									Option.isSome(committed)
										? CompleteReplay(committed.value)
										: Effect.fail(failure),
								),
							),
					),
				);
			});
		const Refresh = (input: WorkspaceGitSessionRefresh) =>
			Schema.decodeUnknownEffect(WorkspaceGitSessionRefresh, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(
					() => new WorkspaceGitObservationError({ reason: "invalid_state" }),
				),
				Effect.flatMap((decoded) =>
					ObserveAndProject({
						kind: "refresh",
						operation_id: decoded.message_id,
						sent_at: decoded.sent_at,
						source_command: true,
						thread_id: decoded.thread_id,
						workspace_id: decoded.workspace_id,
					}),
				),
			);
		const Project = (input: WorkspaceGitProjection) =>
			Schema.decodeUnknownEffect(WorkspaceGitProjection, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(
					() => new WorkspaceGitObservationError({ reason: "invalid_state" }),
				),
				Effect.flatMap((decoded) =>
					ReplayOrObserveAndProject({
						...decoded,
						source_command: false,
					}),
				),
			);
		const ProjectObserved = (
			input: WorkspaceGitProjection,
			observation: WorkspaceGitObservation,
		) =>
			Schema.decodeUnknownEffect(WorkspaceGitProjection, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(
					() => new WorkspaceGitObservationError({ reason: "invalid_state" }),
				),
				Effect.flatMap((decoded) =>
					observation.workspace_id === decoded.workspace_id
						? CommitObservation({ ...decoded, source_command: false }, observation)
						: Effect.fail(
								new WorkspaceGitObservationError({ reason: "invalid_state" }),
							),
				),
			);
		const RecoverEvidence = RecordPendingEvidence();

		yield* RecoverEvidence;

		return {
			Project,
			ProjectObserved,
			Query: repository.Query,
			RecoverEvidence,
			Refresh,
		};
	}),
);
