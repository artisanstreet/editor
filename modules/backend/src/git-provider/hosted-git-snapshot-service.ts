import { Context, Crypto, Data, Effect, Encoding, Layer, Option, Schema } from "effect";

import {
	HostedGitPullRequestLookup,
	HostedGitSnapshotQuery,
	HostedGitSnapshotQueryResult,
	Identifier,
	IsoDateTime,
	type HostedGitSnapshotQueryResult as HostedGitSnapshotQueryResultValue,
} from "@artisan/protocol";

import {
	HostedGitSnapshotRepository,
	type HostedGitSnapshotAcceptance,
	type HostedGitSnapshotRepositoryError,
} from "./hosted-git-snapshot-repository";
import { GitProviderError } from "./git-provider";
import { GitProviderRegistry } from "./git-provider-registry";
import { ProjectRepository } from "../projects/project-repository";
import { WorkspaceGitObserver, type WorkspaceGitObservation } from "../git/workspace-git-observer";

const HostedGitSnapshotRefresh = Schema.Struct({
	message_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

/** Supplies one frontend refresh command for a registered hosted workspace. */
export type HostedGitSnapshotRefresh = typeof HostedGitSnapshotRefresh.Type;

/** Conceals project, provider, and live-workspace failures behind a stable service boundary. */
export class HostedGitSnapshotServiceFailure extends Data.TaggedError(
	"HostedGitSnapshotServiceFailure",
)<{
	readonly reason:
		| "branch_changed"
		| "invalid_provider_response"
		| "invalid_request"
		| "project_unavailable"
		| "provider_unavailable"
		| "workspace_unavailable";
}> {}

/** Represents failures surfaced while reading or projecting hosted review and CI state. */
export type HostedGitSnapshotServiceError =
	| GitProviderError
	| HostedGitSnapshotRepositoryError
	| HostedGitSnapshotServiceFailure;

/** Coordinates exact-head hosted reads with durable snapshot projection and stale-state fencing. */
export class HostedGitSnapshotService extends Context.Service<
	HostedGitSnapshotService,
	{
		readonly Query: (
			query: typeof HostedGitSnapshotQuery.Type,
		) => Effect.Effect<HostedGitSnapshotQueryResultValue, HostedGitSnapshotServiceError>;
		readonly Refresh: (
			input: HostedGitSnapshotRefresh,
		) => Effect.Effect<HostedGitSnapshotAcceptance, HostedGitSnapshotServiceError>;
	}
>()("Artisan/HostedGitSnapshotService") {}

function service_failure(reason: HostedGitSnapshotServiceFailure["reason"]) {
	return new HostedGitSnapshotServiceFailure({ reason });
}

function refresh_fingerprint(input: {
	readonly message_id: string;
	readonly project_id: string;
	readonly sent_at: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}) {
	return JSON.stringify({
		message_id: input.message_id,
		project_id: input.project_id,
		sent_at: input.sent_at,
		thread_id: input.thread_id,
		type: "hosted.git.snapshot.refresh",
		workspace_id: input.workspace_id,
	});
}

function ready_identity(observation: WorkspaceGitObservation, canonical_root: string) {
	return observation.state === "ready" &&
		observation.branch !== undefined &&
		observation.head !== undefined &&
		observation.repository_root === canonical_root &&
		observation.selected_worktree_path === canonical_root
		? {
				branch: observation.branch,
				head: observation.head,
				repository_root: observation.repository_root,
			}
		: undefined;
}

/** Supplies the exact-head hosted projection workflow over local Git and provider adapters. */
export const HostedGitSnapshotServiceLive = Layer.effect(
	HostedGitSnapshotService,
	Effect.gen(function* () {
		const crypto = yield* Crypto.Crypto;
		const observer = yield* WorkspaceGitObserver;
		const projects = yield* ProjectRepository;
		const providers = yield* GitProviderRegistry;
		const repository = yield* HostedGitSnapshotRepository;

		const FindProject = (workspace_id: string) =>
			projects.FindByWorkspaceId({ workspace_id }).pipe(
				Effect.mapError(() => service_failure("project_unavailable")),
				Effect.flatMap(
					Option.match({
						onNone: () => Effect.fail(service_failure("project_unavailable")),
						onSome: Effect.succeed,
					}),
				),
			);

		const Fingerprint = (input: Parameters<typeof refresh_fingerprint>[0]) =>
			crypto.digest("SHA-256", new TextEncoder().encode(refresh_fingerprint(input))).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(() => service_failure("invalid_request")),
			);

		const ObserveReady = (workspace_id: string, canonical_root: string) =>
			observer.Observe(workspace_id).pipe(
				Effect.mapError(() => service_failure("workspace_unavailable")),
				Effect.flatMap((observation) => {
					const identity = ready_identity(observation, canonical_root);

					return identity === undefined
						? Effect.fail(service_failure("workspace_unavailable"))
						: Effect.succeed({ identity, observation });
				}),
			);

		const ValidateLookup = (
			lookup: unknown,
			expected: {
				readonly branch: string;
				readonly head: string;
				readonly host: string;
				readonly name: string;
				readonly owner: string;
				readonly provider_id: string;
			},
		) =>
			Schema.decodeUnknownEffect(HostedGitPullRequestLookup, {
				onExcessProperty: "error",
			})(lookup).pipe(
				Effect.mapError(() => service_failure("invalid_provider_response")),
				Effect.flatMap((decoded) =>
					decoded.branch !== expected.branch ||
					decoded.expected_head_commit !== expected.head ||
					decoded.repository.host !== expected.host ||
					decoded.repository.name !== expected.name ||
					decoded.repository.owner !== expected.owner ||
					decoded.repository.provider_id !== expected.provider_id ||
					(decoded.association._tag === "matched" &&
						(decoded.association.freshness !== "current" ||
							decoded.association.pull_request.head_branch !== expected.branch ||
							decoded.association.pull_request.head_commit !== expected.head))
						? Effect.fail(service_failure("invalid_provider_response"))
						: Effect.succeed(decoded),
				),
			);

		const Refresh = (input: HostedGitSnapshotRefresh) =>
			Schema.decodeUnknownEffect(HostedGitSnapshotRefresh, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => service_failure("invalid_request")),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const project = yield* FindProject(decoded.workspace_id);
						const request_fingerprint = yield* Fingerprint({
							message_id: decoded.message_id,
							project_id: project.project.project_id,
							sent_at: decoded.sent_at,
							thread_id: decoded.thread_id,
							workspace_id: decoded.workspace_id,
						});
						const replay_input = {
							operation_id: decoded.message_id,
							project_id: project.project.project_id,
							request_fingerprint,
							sent_at: decoded.sent_at,
							thread_id: decoded.thread_id,
							workspace_id: decoded.workspace_id,
						};
						const replay = yield* repository.Replay(replay_input);

						if (Option.isSome(replay)) {
							return replay.value;
						}

						const before = yield* ObserveReady(
							decoded.workspace_id,
							project.project.root_path,
						);
						const provider = yield* providers
							.Get(project.hosted_origin.provider_id)
							.pipe(Effect.mapError(() => service_failure("provider_unavailable")));
						const ReadPullRequest = provider.ReadPullRequest;

						if (ReadPullRequest === undefined) {
							return yield* service_failure("provider_unavailable");
						}

						const lookup = yield* ReadPullRequest({
							expected_head: before.identity.head,
							repository: {
								host: project.hosted_origin.canonical_host,
								name: project.hosted_origin.name,
								owner: project.hosted_origin.owner,
								provider_id: project.hosted_origin.provider_id,
							},
							selected_branch: before.identity.branch,
							selection: {
								account_login: project.hosted_origin.selected_account_login,
								host: project.hosted_origin.canonical_host,
								provider_id: project.hosted_origin.provider_id,
							},
						});
						const canonical_lookup = yield* ValidateLookup(lookup, {
							...before.identity,
							host: project.hosted_origin.canonical_host,
							name: project.hosted_origin.name,
							owner: project.hosted_origin.owner,
							provider_id: project.hosted_origin.provider_id,
						});
						const after = yield* ObserveReady(
							decoded.workspace_id,
							project.project.root_path,
						);

						if (
							after.identity.branch !== before.identity.branch ||
							after.identity.head !== before.identity.head ||
							after.identity.repository_root !== before.identity.repository_root
						) {
							return yield* service_failure("branch_changed");
						}

						return yield* repository
							.Project({
								lookup: canonical_lookup,
								observed_at: after.observation.observed_at,
								operation_id: decoded.message_id,
								project_id: project.project.project_id,
								request_fingerprint,
								source_command: {
									message_id: decoded.message_id,
									sent_at: decoded.sent_at,
								},
								thread_id: decoded.thread_id,
								workspace_id: decoded.workspace_id,
							})
							.pipe(
								Effect.catch((failure) =>
									repository
										.Replay(replay_input)
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

		const Query = (query: typeof HostedGitSnapshotQuery.Type) =>
			Schema.decodeUnknownEffect(HostedGitSnapshotQuery, {
				onExcessProperty: "error",
			})(query).pipe(
				Effect.mapError(() => service_failure("invalid_request")),
				Effect.flatMap((decoded) =>
					repository.Query(decoded).pipe(
						Effect.flatMap((stored) => {
							const snapshot = stored.snapshot;

							if (snapshot === undefined) {
								return Effect.succeed(stored);
							}

							return Effect.gen(function* () {
								const project = yield* FindProject(decoded.workspace_id);
								const observation = yield* observer
									.Observe(decoded.workspace_id)
									.pipe(Effect.option);
								const identity = Option.isSome(observation)
									? ready_identity(observation.value, project.project.root_path)
									: undefined;
								const current =
									identity !== undefined &&
									identity.branch === snapshot.lookup.branch &&
									identity.head === snapshot.lookup.expected_head_commit;
								const result = {
									...stored,
									snapshot: {
										...snapshot,
										workspace_freshness: current
											? "current"
											: "stale_local_git",
									},
								};

								return yield* Schema.decodeUnknownEffect(
									HostedGitSnapshotQueryResult,
									{ onExcessProperty: "error" },
								)(result).pipe(
									Effect.mapError(() =>
										service_failure("invalid_provider_response"),
									),
								);
							});
						}),
					),
				),
			);

		return { Query, Refresh };
	}),
);
