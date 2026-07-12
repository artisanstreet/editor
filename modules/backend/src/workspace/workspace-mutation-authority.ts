import { eq, or } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	AssignmentPermissionPolicy,
	AssignmentScope,
	AssignmentWorkspace,
	ContentIdentity,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspacePath,
} from "@artisan/protocol";

import {
	WorkspaceFilesystemAuthorizationError,
	WorkspaceFilesystemNotFoundError,
	WorkspaceFilesystemRegistry,
	type WorkspaceFilesystem,
} from "../filesystem/workspace-filesystem-registry";
import { Database } from "../persistence/database";
import { JournalStoreFailure, CommandIdConflict } from "../persistence/journal-store";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	OrchestrationCoordinators,
	OrchestrationGroups,
	OrchestrationRuns,
	WorkspaceChangeOperations,
	WorkspaceMutationAuthorities,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import {
	WorkspaceChangeIdConflict,
	WorkspaceChangeRepository,
	WorkspaceChangeTransitionError,
	type ClaimReplace,
	type WorkspaceChangeClaim,
} from "./workspace-change-repository";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));
const WorkspaceMutationAuthorityKind = Schema.Literals(["base_run", "graph_run"]);

const WorkspaceMutationClaimReplace = Schema.Struct({
	_tag: Schema.Literal("replace"),
	agent_id: Identifier,
	authority: WorkspaceMutationAuthorityKind,
	change_id: Identifier,
	expected_before: ContentIdentity,
	intended_after: ContentIdentity,
	message_id: Identifier,
	path: WorkspacePath,
	raw_origin: Schema.optional(RawOrigin),
	request_fingerprint: RequestFingerprint,
	run_id: Identifier,
	sent_at: IsoDateTime,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const StoredAuthorityBase = {
	agent_id: Identifier,
	change_id: Identifier,
	created_at: IsoDateTime,
	message_id: Identifier,
	run_id: Identifier,
	thread_id: Identifier,
	working_directory: Schema.NonEmptyString,
	workspace_id: Identifier,
};

const StoredWorkspaceMutationAuthority = Schema.Union([
	Schema.Struct({
		...StoredAuthorityBase,
		approval: Schema.Null,
		assignment_id: Schema.Null,
		authority_kind: Schema.Literal("base_run"),
		group_id: Schema.Null,
		scope_kind: Schema.Null,
		scope_value: Schema.Null,
	}),
	Schema.Struct({
		...StoredAuthorityBase,
		approval: Schema.Literals(["never", "on_request", "always"]),
		assignment_id: Identifier,
		authority_kind: Schema.Literal("graph_run"),
		group_id: Identifier,
		scope_kind: Schema.Literals(["repo", "files"]),
		scope_value: Schema.NonEmptyString,
	}),
]);

/** Identifies the single supported controlled-mutation claim. */
export type WorkspaceMutationClaimReplace = typeof WorkspaceMutationClaimReplace.Type;

/** Describes the durable authority that admitted a controlled mutation. */
export type WorkspaceMutationAuthorityGrant =
	| {
			readonly _tag: "base_run";
			readonly agent_id: string;
			readonly run_id: string;
			readonly thread_id: string;
			readonly workspace_id: string;
	  }
	| {
			readonly _tag: "graph_run";
			readonly agent_id: string;
			readonly approval: "never" | "on_request" | "always";
			readonly assignment_id: string;
			readonly group_id: string;
			readonly run_id: string;
			readonly scope: "repo" | "files";
			readonly thread_id: string;
			readonly workspace_id: string;
	  };

/** Returns an atomic mutation claim with its confined filesystem capability. */
export interface WorkspaceMutationAdmission {
	readonly authority: WorkspaceMutationAuthorityGrant;
	readonly claim: WorkspaceChangeClaim;
	readonly filesystem: WorkspaceFilesystem;
}

export type WorkspaceMutationAuthorityDenialReason =
	| "identity_mismatch"
	| "path_outside_scope"
	| "run_not_active"
	| "thread_unavailable"
	| "unsupported_scope"
	| "workspace_mismatch"
	| "workspace_unavailable"
	| "write_not_allowed";

/** Reports a malformed controlled-mutation request without echoing its content. */
export class WorkspaceMutationAuthorityInvalid extends Data.TaggedError(
	"WorkspaceMutationAuthorityInvalid",
)<{ readonly operation: "claim_replace" }> {}

/** Reports a live authority proof that does not permit the requested mutation. */
export class WorkspaceMutationAuthorityDenied extends Data.TaggedError(
	"WorkspaceMutationAuthorityDenied",
)<{ readonly reason: WorkspaceMutationAuthorityDenialReason }> {}

/** Reports a reused operation whose durable state prevents mutation admission. */
export class WorkspaceMutationAuthorityConflict extends Data.TaggedError(
	"WorkspaceMutationAuthorityConflict",
)<{
	readonly reason:
		| "authority_conflict"
		| "operation_conflict"
		| "operation_rejected"
		| "unpinned_operation";
}> {}

/** Conceals corrupt state and unexpected persistence failures from mutation callers. */
export class WorkspaceMutationAuthorityFailure extends Data.TaggedError(
	"WorkspaceMutationAuthorityFailure",
)<{ readonly reason: "invalid_persisted_state" | "persistence_failure" }> {}

export type WorkspaceMutationAuthorityError =
	| WorkspaceMutationAuthorityConflict
	| WorkspaceMutationAuthorityDenied
	| WorkspaceMutationAuthorityFailure
	| WorkspaceMutationAuthorityInvalid;

/** Atomically proves run authority and claims one controlled workspace mutation. */
export class WorkspaceMutationAuthority extends Context.Service<
	WorkspaceMutationAuthority,
	{
		readonly ClaimReplace: (
			input: WorkspaceMutationClaimReplace,
		) => Effect.Effect<WorkspaceMutationAdmission, WorkspaceMutationAuthorityError>;
	}
>()("Artisan/WorkspaceMutationAuthority") {}

type StoredAuthority = typeof StoredWorkspaceMutationAuthority.Type;

interface AuthorityProof {
	readonly authority: WorkspaceMutationAuthorityGrant;
	readonly filesystem: WorkspaceFilesystem;
	readonly stored: Omit<typeof WorkspaceMutationAuthorities.$inferInsert, "created_at">;
}

function invalid_state() {
	return new WorkspaceMutationAuthorityFailure({ reason: "invalid_persisted_state" });
}

function denied(reason: WorkspaceMutationAuthorityDenialReason) {
	return new WorkspaceMutationAuthorityDenied({ reason });
}

function DecodeClaim(input: unknown) {
	return Schema.decodeUnknownEffect(WorkspaceMutationClaimReplace, {
		onExcessProperty: "error",
	})(input).pipe(
		Effect.mapError(
			() => new WorkspaceMutationAuthorityInvalid({ operation: "claim_replace" }),
		),
	);
}

function DecodeStoredJson<A>(schema: Schema.Codec<A, A>, value: string) {
	return Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
		Effect.mapError(invalid_state),
	);
}

function DecodeStoredAuthority(row: unknown) {
	return Schema.decodeUnknownEffect(StoredWorkspaceMutationAuthority, {
		onExcessProperty: "error",
	})(row).pipe(Effect.mapError(invalid_state));
}

function grant_from_stored(authority: StoredAuthority): WorkspaceMutationAuthorityGrant {
	if (authority.authority_kind === "base_run") {
		return {
			_tag: "base_run",
			agent_id: authority.agent_id,
			run_id: authority.run_id,
			thread_id: authority.thread_id,
			workspace_id: authority.workspace_id,
		};
	}

	return {
		_tag: "graph_run",
		agent_id: authority.agent_id,
		approval: authority.approval,
		assignment_id: authority.assignment_id,
		group_id: authority.group_id,
		run_id: authority.run_id,
		scope: authority.scope_kind,
		thread_id: authority.thread_id,
		workspace_id: authority.workspace_id,
	};
}

function authority_matches_claim(authority: StoredAuthority, claim: WorkspaceMutationClaimReplace) {
	return (
		authority.agent_id === claim.agent_id &&
		authority.authority_kind === claim.authority &&
		authority.change_id === claim.change_id &&
		authority.message_id === claim.message_id &&
		authority.run_id === claim.run_id &&
		authority.thread_id === claim.thread_id &&
		authority.workspace_id === claim.workspace_id
	);
}

function file_scope_contains(scope: string, path: string) {
	return path === scope || path.startsWith(`${scope}/`);
}

function repository_claim_from(claim: WorkspaceMutationClaimReplace): ClaimReplace {
	return {
		_tag: "replace",
		agent_id: claim.agent_id,
		change_id: claim.change_id,
		expected_before: claim.expected_before,
		intended_after: claim.intended_after,
		message_id: claim.message_id,
		path: claim.path,
		...(claim.raw_origin === undefined ? {} : { raw_origin: claim.raw_origin }),
		request_fingerprint: claim.request_fingerprint,
		run_id: claim.run_id,
		sent_at: claim.sent_at,
		thread_id: claim.thread_id,
		workspace_id: claim.workspace_id,
	};
}

function conceal_error(error: unknown): WorkspaceMutationAuthorityError {
	if (
		error instanceof WorkspaceMutationAuthorityConflict ||
		error instanceof WorkspaceMutationAuthorityDenied ||
		error instanceof WorkspaceMutationAuthorityFailure ||
		error instanceof WorkspaceMutationAuthorityInvalid
	) {
		return error;
	}

	if (
		error instanceof WorkspaceFilesystemAuthorizationError ||
		error instanceof WorkspaceFilesystemNotFoundError
	) {
		return denied("workspace_unavailable");
	}

	if (error instanceof CommandIdConflict || error instanceof WorkspaceChangeIdConflict) {
		return new WorkspaceMutationAuthorityConflict({ reason: "operation_conflict" });
	}

	if (error instanceof WorkspaceChangeTransitionError) {
		return denied("thread_unavailable");
	}

	return new WorkspaceMutationAuthorityFailure({ reason: "persistence_failure" });
}

/** Supplies the SQLite-backed transactional mutation authority. */
export const WorkspaceMutationAuthorityLive = Layer.effect(
	WorkspaceMutationAuthority,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const registry = yield* WorkspaceFilesystemRegistry;
		const repository = yield* WorkspaceChangeRepository;

		const AuthorizeWorkspace = (workspace_id: string, working_directory: string) =>
			registry
				.Authorize({ working_directory, workspace_id })
				.pipe(Effect.mapError(() => denied("workspace_unavailable")));
		const ValidatePinnedScope = (
			authority: StoredAuthority,
			claim: WorkspaceMutationClaimReplace,
		) =>
			Effect.gen(function* () {
				if (authority.authority_kind === "base_run") {
					return;
				}

				if (authority.scope_kind === "repo") {
					yield* AuthorizeWorkspace(authority.workspace_id, authority.scope_value);

					return;
				}

				const controlled_scope = yield* Schema.decodeUnknownEffect(WorkspacePath)(
					authority.scope_value,
				).pipe(Effect.mapError(invalid_state));

				if (!file_scope_contains(controlled_scope, claim.path)) {
					return yield* Effect.fail(denied("path_outside_scope"));
				}
			});
		/** A same-value update makes exact retries serialize against thread erasure. */
		const FenceExactRetry = (transaction: typeof database.client, authority: StoredAuthority) =>
			transaction
				.update(WorkspaceMutationAuthorities)
				.set({ created_at: authority.created_at })
				.where(eq(WorkspaceMutationAuthorities.message_id, authority.message_id))
				.returning({ message_id: WorkspaceMutationAuthorities.message_id })
				.pipe(
					Effect.flatMap(([fenced]) =>
						fenced ? Effect.void : Effect.fail(invalid_state()),
					),
				);

		const ProveBaseRun = (
			transaction: typeof database.client,
			claim: WorkspaceMutationClaimReplace,
		) =>
			Effect.gen(function* () {
				const [run] = yield* transaction
					.select()
					.from(OrchestrationRuns)
					.where(eq(OrchestrationRuns.run_id, claim.run_id))
					.limit(1);
				const [coordinator] = yield* transaction
					.select()
					.from(OrchestrationCoordinators)
					.where(eq(OrchestrationCoordinators.thread_id, claim.thread_id))
					.limit(1);

				if (
					!run ||
					!coordinator ||
					coordinator.active_run_id !== claim.run_id ||
					(run.status !== "running" && run.status !== "waiting")
				) {
					return yield* Effect.fail(denied("run_not_active"));
				}

				if (
					coordinator.agent_id !== claim.agent_id ||
					run.agent_id !== claim.agent_id ||
					run.thread_id !== claim.thread_id
				) {
					return yield* Effect.fail(denied("identity_mismatch"));
				}

				const authorized = yield* AuthorizeWorkspace(
					claim.workspace_id,
					run.working_directory,
				);

				return {
					authority: {
						_tag: "base_run",
						agent_id: claim.agent_id,
						run_id: claim.run_id,
						thread_id: claim.thread_id,
						workspace_id: claim.workspace_id,
					},
					filesystem: authorized.filesystem,
					stored: {
						agent_id: claim.agent_id,
						approval: null,
						assignment_id: null,
						authority_kind: "base_run",
						change_id: claim.change_id,
						group_id: null,
						message_id: claim.message_id,
						run_id: claim.run_id,
						scope_kind: null,
						scope_value: null,
						thread_id: claim.thread_id,
						working_directory: run.working_directory,
						workspace_id: claim.workspace_id,
					},
				} satisfies AuthorityProof;
			});

		const ProveGraphRun = (
			transaction: typeof database.client,
			claim: WorkspaceMutationClaimReplace,
		) =>
			Effect.gen(function* () {
				const [run] = yield* transaction
					.select()
					.from(AgentRuns)
					.where(eq(AgentRuns.run_id, claim.run_id))
					.limit(1);

				if (!run) {
					return yield* Effect.fail(denied("run_not_active"));
				}

				const [assignment] = yield* transaction
					.select()
					.from(Assignments)
					.where(eq(Assignments.assignment_id, run.assignment_id))
					.limit(1);
				const [group] = yield* transaction
					.select()
					.from(OrchestrationGroups)
					.where(eq(OrchestrationGroups.group_id, run.group_id))
					.limit(1);
				const [agent] = yield* transaction
					.select()
					.from(AgentInstances)
					.where(eq(AgentInstances.agent_id, run.agent_id))
					.limit(1);

				if (
					!assignment ||
					!group ||
					!agent ||
					assignment.active_run_id !== run.run_id ||
					group.state !== "running" ||
					run.dispatch_status !== "active" ||
					(run.state !== "running" && run.state !== "waiting") ||
					(assignment.state !== "running" && assignment.state !== "waiting")
				) {
					return yield* Effect.fail(denied("run_not_active"));
				}

				if (
					agent.agent_id !== claim.agent_id ||
					agent.group_id !== run.group_id ||
					assignment.agent_id !== claim.agent_id ||
					assignment.group_id !== run.group_id ||
					group.thread_id !== claim.thread_id ||
					run.agent_id !== claim.agent_id ||
					run.assignment_id !== assignment.assignment_id ||
					run.group_id !== group.group_id
				) {
					return yield* Effect.fail(denied("identity_mismatch"));
				}

				const scope = yield* DecodeStoredJson(AssignmentScope, assignment.scope_json);
				const workspace = yield* DecodeStoredJson(
					AssignmentWorkspace,
					assignment.workspace_json,
				);
				const permission_policy = yield* DecodeStoredJson(
					AssignmentPermissionPolicy,
					assignment.permission_policy_json,
				);

				if (workspace.workspace_id !== claim.workspace_id) {
					return yield* Effect.fail(denied("workspace_mismatch"));
				}

				if (!permission_policy.write_access || !scope.write_access) {
					return yield* Effect.fail(denied("write_not_allowed"));
				}

				if (scope.kind !== "repo" && scope.kind !== "files") {
					return yield* Effect.fail(denied("unsupported_scope"));
				}

				const authorized = yield* AuthorizeWorkspace(
					claim.workspace_id,
					workspace.working_directory,
				);

				if (scope.kind === "repo") {
					yield* AuthorizeWorkspace(claim.workspace_id, scope.value);
				} else {
					const controlled_scope = yield* Schema.decodeUnknownEffect(WorkspacePath)(
						scope.value,
					).pipe(Effect.mapError(() => denied("path_outside_scope")));

					if (!file_scope_contains(controlled_scope, claim.path)) {
						return yield* Effect.fail(denied("path_outside_scope"));
					}
				}

				return {
					authority: {
						_tag: "graph_run",
						agent_id: claim.agent_id,
						approval: permission_policy.approval,
						assignment_id: assignment.assignment_id,
						group_id: group.group_id,
						run_id: claim.run_id,
						scope: scope.kind,
						thread_id: claim.thread_id,
						workspace_id: claim.workspace_id,
					},
					filesystem: authorized.filesystem,
					stored: {
						agent_id: claim.agent_id,
						approval: permission_policy.approval,
						assignment_id: assignment.assignment_id,
						authority_kind: "graph_run",
						change_id: claim.change_id,
						group_id: group.group_id,
						message_id: claim.message_id,
						run_id: claim.run_id,
						scope_kind: scope.kind,
						scope_value: scope.value,
						thread_id: claim.thread_id,
						working_directory: workspace.working_directory,
						workspace_id: claim.workspace_id,
					},
				} satisfies AuthorityProof;
			});

		const ClaimRepositoryReplace = (claim: WorkspaceMutationClaimReplace) =>
			repository
				.ClaimReplace(repository_claim_from(claim))
				.pipe(
					Effect.mapError((error) =>
						error instanceof JournalStoreFailure ? error.cause : error,
					),
				);

		const ClaimReplace = (input: WorkspaceMutationClaimReplace) =>
			DecodeClaim(input).pipe(
				Effect.flatMap((claim) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const existing_authorities = yield* transaction
									.select()
									.from(WorkspaceMutationAuthorities)
									.where(
										or(
											eq(
												WorkspaceMutationAuthorities.message_id,
												claim.message_id,
											),
											eq(
												WorkspaceMutationAuthorities.change_id,
												claim.change_id,
											),
										),
									)
									.limit(2);
								const [existing_operation] = yield* transaction
									.select({
										change_id: WorkspaceChangeOperations.change_id,
										message_id: WorkspaceChangeOperations.message_id,
									})
									.from(WorkspaceChangeOperations)
									.where(
										eq(WorkspaceChangeOperations.message_id, claim.message_id),
									)
									.limit(1);

								if (existing_authorities.length > 1) {
									return yield* Effect.fail(invalid_state());
								}

								const existing_authority = existing_authorities[0]
									? yield* DecodeStoredAuthority(existing_authorities[0])
									: undefined;

								if (existing_operation) {
									if (!existing_authority) {
										return yield* Effect.fail(
											new WorkspaceMutationAuthorityConflict({
												reason: "unpinned_operation",
											}),
										);
									}

									if (!authority_matches_claim(existing_authority, claim)) {
										return yield* Effect.fail(
											new WorkspaceMutationAuthorityConflict({
												reason: "authority_conflict",
											}),
										);
									}

									yield* ValidatePinnedScope(existing_authority, claim);

									const authorized = yield* AuthorizeWorkspace(
										existing_authority.workspace_id,
										existing_authority.working_directory,
									);

									yield* FenceExactRetry(transaction, existing_authority);

									const accepted = yield* ClaimRepositoryReplace(claim);

									if (accepted._tag === "rejected") {
										return yield* Effect.fail(
											new WorkspaceMutationAuthorityConflict({
												reason: "operation_rejected",
											}),
										);
									}

									return {
										authority: grant_from_stored(existing_authority),
										claim: accepted,
										filesystem: authorized.filesystem,
									};
								}

								if (existing_authority) {
									return yield* Effect.fail(
										new WorkspaceMutationAuthorityConflict({
											reason: "authority_conflict",
										}),
									);
								}

								const proof = yield* claim.authority === "base_run"
									? ProveBaseRun(transaction, claim)
									: ProveGraphRun(transaction, claim);
								const created_at = yield* metadata.Now;
								const accepted = yield* ClaimRepositoryReplace(claim);

								if (accepted._tag !== "claimed") {
									return yield* Effect.fail(invalid_state());
								}

								yield* transaction
									.insert(WorkspaceMutationAuthorities)
									.values({ ...proof.stored, created_at });

								return {
									authority: proof.authority,
									claim: accepted,
									filesystem: proof.filesystem,
								};
							}),
						),
					),
				),
				Effect.mapError(conceal_error),
			);

		return { ClaimReplace };
	}),
);
