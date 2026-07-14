import { eq } from "drizzle-orm";
import { Context, Crypto, Data, Effect, Encoding, Layer, Option, Schema } from "effect";

import {
	ExternalWaitCancelRequest,
	ExternalWaitManualResumeRequest,
	ExternalWaitOwner,
	ExternalWaitQuery,
	ExternalWaitRequest,
	ExternalWaitTarget,
	Identifier,
	IsoDateTime,
	type ExternalWaitGate,
	type ExternalWaitQueryResult,
} from "@artisan/protocol";

import { BuildExternalWaitBaseline, type ExternalWaitPolicyError } from "./external-wait-policy";
import {
	ExternalWaitRepository,
	type ExternalWaitAcceptance,
	type ExternalWaitManualResumeAcceptance,
	type ExternalWaitRepositoryError,
} from "./external-wait-repository";
import { ExternalWaitDispatcher } from "./external-wait-dispatcher";
import {
	HostedGitSnapshotService,
	type HostedGitSnapshotServiceError,
} from "../git-provider/hosted-git-snapshot-service";
import { Database } from "../persistence/database";
import { AgentRuns, OrchestrationGroups, OrchestrationRuns } from "../persistence/schema";

const ExternalWaitRequestCommand = Schema.Struct({
	...ExternalWaitRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

const ExternalWaitCancelCommand = Schema.Struct({
	...ExternalWaitCancelRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

const ExternalWaitManualResumeCommand = Schema.Struct({
	...ExternalWaitManualResumeRequest.fields,
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
});

export type ExternalWaitRequestCommand = typeof ExternalWaitRequestCommand.Type;
export type ExternalWaitCancelCommand = typeof ExternalWaitCancelCommand.Type;
export type ExternalWaitManualResumeCommand = typeof ExternalWaitManualResumeCommand.Type;

/** Reports an application-level external-wait request that cannot be satisfied safely. */
export class ExternalWaitServiceFailure extends Data.TaggedError("ExternalWaitServiceFailure")<{
	readonly reason:
		| "already_satisfied"
		| "invalid_request"
		| "persistence_unavailable"
		| "snapshot_stale"
		| "snapshot_unavailable"
		| "source_run_unavailable"
		| "wait_unavailable";
}> {}

export type ExternalWaitServiceError =
	| ExternalWaitPolicyError
	| ExternalWaitRepositoryError
	| ExternalWaitServiceFailure
	| HostedGitSnapshotServiceError;

/** Derives durable external-wait ownership and hosted evidence for public control operations. */
export class ExternalWaitService extends Context.Service<
	ExternalWaitService,
	{
		readonly Cancel: (
			input: ExternalWaitCancelCommand,
		) => Effect.Effect<ExternalWaitAcceptance, ExternalWaitServiceError>;
		readonly ManualResume: (
			input: ExternalWaitManualResumeCommand,
		) => Effect.Effect<ExternalWaitManualResumeAcceptance, ExternalWaitServiceError>;
		readonly Query: (
			input: typeof ExternalWaitQuery.Type,
		) => Effect.Effect<ExternalWaitQueryResult, ExternalWaitServiceError>;
		readonly Request: (
			input: ExternalWaitRequestCommand,
		) => Effect.Effect<ExternalWaitAcceptance, ExternalWaitServiceError>;
	}
>()("Artisan/ExternalWaitService") {}

function service_failure(reason: ExternalWaitServiceFailure["reason"]) {
	return new ExternalWaitServiceFailure({ reason });
}

function compare_strings(left: string, right: string) {
	return left < right ? -1 : left > right ? 1 : 0;
}

function canonical_gate(gate: ExternalWaitGate): ExternalWaitGate {
	return gate._tag === "selected_checks_terminal"
		? { ...gate, check_names: [...gate.check_names].sort(compare_strings) }
		: gate;
}

function request_fingerprint_frame(input: ExternalWaitRequestCommand) {
	return JSON.stringify({
		expected_head_commit: input.expected_head_commit,
		gates: input.gates
			.map(canonical_gate)
			.sort((left, right) => compare_strings(JSON.stringify(left), JSON.stringify(right))),
		pull_request_number: input.pull_request_number,
		source_run_id: input.source_run_id,
		thread_id: input.thread_id,
		type: "external_wait.request",
		workspace_id: input.workspace_id,
	});
}

/** Supplies the public external-wait workflow over hosted projections and durable orchestration. */
export const ExternalWaitServiceLive = Layer.effect(
	ExternalWaitService,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const database = yield* Database;
		const dispatcher = yield* ExternalWaitDispatcher;
		const hosted_git = yield* HostedGitSnapshotService;
		const repository = yield* ExternalWaitRepository;

		const FingerprintRequest = (input: ExternalWaitRequestCommand) =>
			crypto
				.digest("SHA-256", new TextEncoder().encode(request_fingerprint_frame(input)))
				.pipe(
					Effect.map(Encoding.encodeHex),
					Effect.mapError(() => service_failure("invalid_request")),
				);

		const ResolveOwner = (source_run_id: string, thread_id: string) =>
			Effect.gen(function* () {
				const [ordinary_runs, assignment_runs] = yield* Effect.all([
					database.client
						.select({
							agent_id: OrchestrationRuns.agent_id,
							engine_id: OrchestrationRuns.engine_id,
							run_id: OrchestrationRuns.run_id,
							thread_id: OrchestrationRuns.thread_id,
						})
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, source_run_id))
						.limit(2),
					database.client
						.select({
							agent_id: AgentRuns.agent_id,
							assignment_id: AgentRuns.assignment_id,
							engine_id: AgentRuns.engine_id,
							group_id: AgentRuns.group_id,
							run_id: AgentRuns.run_id,
							thread_id: OrchestrationGroups.thread_id,
						})
						.from(AgentRuns)
						.innerJoin(
							OrchestrationGroups,
							eq(AgentRuns.group_id, OrchestrationGroups.group_id),
						)
						.where(eq(AgentRuns.run_id, source_run_id))
						.limit(2),
				]);
				const owners = [
					...ordinary_runs.map((run) => ({
						_tag: "thread_run" as const,
						agent_id: run.agent_id,
						engine_id: run.engine_id,
						run_id: run.run_id,
						thread_id: run.thread_id,
					})),
					...assignment_runs.map((run) => ({
						_tag: "assignment_run" as const,
						agent_id: run.agent_id,
						assignment_id: run.assignment_id,
						engine_id: run.engine_id,
						group_id: run.group_id,
						run_id: run.run_id,
						thread_id: run.thread_id,
					})),
				];
				const owner = owners[0];

				if (owners.length !== 1 || owner === undefined || owner.thread_id !== thread_id) {
					return yield* service_failure("source_run_unavailable");
				}

				const { thread_id: _thread_id, ...public_owner } = owner;

				return yield* Schema.decodeUnknownEffect(ExternalWaitOwner, {
					onExcessProperty: "error",
				})(public_owner).pipe(
					Effect.mapError(() => service_failure("source_run_unavailable")),
				);
			}).pipe(
				Effect.mapError((error) =>
					error instanceof ExternalWaitServiceFailure
						? error
						: service_failure("persistence_unavailable"),
				),
			);

		const Request = (input: ExternalWaitRequestCommand) =>
			Schema.decodeUnknownEffect(ExternalWaitRequestCommand, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => service_failure("invalid_request")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const request_fingerprint = yield* FingerprintRequest(decoded);
						const source_command = {
							message_id: decoded.message_id,
							sent_at: decoded.sent_at,
						};
						const replay_input = {
							request_fingerprint,
							source_command,
							thread_id: decoded.thread_id,
							wait_id: decoded.message_id,
						};
						const replay = yield* repository.ReplayRequest(replay_input);

						if (Option.isSome(replay)) {
							return replay.value;
						}

						const snapshot = yield* hosted_git.ReadCurrent({
							workspace_id: decoded.workspace_id,
						});

						if (snapshot.lookup.association._tag !== "matched") {
							return yield* service_failure("snapshot_unavailable");
						}

						const pull_request = snapshot.lookup.association.pull_request;
						const target = yield* Schema.decodeUnknownEffect(ExternalWaitTarget, {
							onExcessProperty: "error",
						})({
							branch: snapshot.lookup.branch,
							expected_head_commit: snapshot.lookup.expected_head_commit,
							pull_request_number: pull_request.number,
							pull_request_origin: pull_request.origin,
							repository: snapshot.lookup.repository,
						}).pipe(Effect.mapError(() => service_failure("snapshot_unavailable")));

						if (
							decoded.expected_head_commit !== target.expected_head_commit ||
							decoded.pull_request_number !== target.pull_request_number
						) {
							return yield* service_failure("snapshot_stale");
						}

						const owner = yield* ResolveOwner(decoded.source_run_id, decoded.thread_id);
						const baseline_result = yield* BuildExternalWaitBaseline({
							gates: decoded.gates,
							lookup: snapshot.lookup,
							target,
						});

						if (baseline_result._tag === "already_satisfied") {
							return yield* service_failure("already_satisfied");
						}

						const registration = {
							baseline: baseline_result.baseline,
							owner,
							project_id: snapshot.project_id,
							request: {
								expected_head_commit: decoded.expected_head_commit,
								gates: decoded.gates,
								pull_request_number: decoded.pull_request_number,
								source_run_id: decoded.source_run_id,
								workspace_id: decoded.workspace_id,
							},
							request_fingerprint,
							source_command,
							target,
							thread_id: decoded.thread_id,
							wait_id: decoded.message_id,
						};

						return yield* repository
							.Register(registration)
							.pipe(
								Effect.catch((failure) =>
									repository
										.ReplayRequest(replay_input)
										.pipe(
											Effect.flatMap((committed) =>
												Option.isSome(committed)
													? Effect.succeed(committed.value)
													: Effect.fail(failure),
											),
										),
								),
							);
					}),
				),
			);

		const Cancel = (input: ExternalWaitCancelCommand) =>
			Schema.decodeUnknownEffect(ExternalWaitCancelCommand, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => service_failure("invalid_request")),
				Effect.flatMap((decoded) =>
					repository.Cancel({
						now: decoded.sent_at,
						reason: "user",
						source_command: {
							message_id: decoded.message_id,
							sent_at: decoded.sent_at,
						},
						thread_id: decoded.thread_id,
						wait_id: decoded.wait_id,
					}),
				),
				Effect.flatMap(
					Option.match({
						onNone: () => Effect.fail(service_failure("wait_unavailable")),
						onSome: Effect.succeed,
					}),
				),
			);

		const ManualResume = (input: ExternalWaitManualResumeCommand) =>
			Schema.decodeUnknownEffect(ExternalWaitManualResumeCommand, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => service_failure("invalid_request")),
				Effect.flatMap((decoded) =>
					repository.ManualResume({
						now: decoded.sent_at,
						source_command: {
							message_id: decoded.message_id,
							sent_at: decoded.sent_at,
						},
						thread_id: decoded.thread_id,
						wait_id: decoded.wait_id,
					}),
				),
				Effect.tap(() => dispatcher.RunOnce.pipe(Effect.catch(() => Effect.void))),
			);

		const Query = (input: typeof ExternalWaitQuery.Type) =>
			Schema.decodeUnknownEffect(ExternalWaitQuery, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(() => service_failure("invalid_request")),
				Effect.flatMap(repository.Query),
			);

		return { Cancel, ManualResume, Query, Request };
	}),
);
