import { and, eq, or } from "drizzle-orm";
import { Effect, Layer, Schema } from "effect";

import {
	AssignmentPermissionPolicy,
	AssignmentScope,
	AssignmentWorkspace,
	WorkspacePath,
} from "@artisan/protocol";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../../filesystem/workspace-bounded-regular-file-store-registry";
import { Database } from "../../persistence/database";
import { JournalStoreFailure } from "../../persistence/journal-store";
import { RetrySqliteWrite } from "../../persistence/sqlite-write-retry";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	OrchestrationCoordinators,
	OrchestrationGroups,
	OrchestrationRuns,
	WorkspaceChanges,
	WorkspaceChangeOperations,
	WorkspaceMutationAuthorities,
} from "../../persistence/tables";
import { RuntimeMetadata } from "../../runtime/metadata";
import { WorkspaceChangeRepository, WorkspaceChangeTransitionError } from "../changes/repository";
import {
	type AuthorityProof,
	WorkspaceMutationAuthority,
	WorkspaceMutationAuthorityConflict,
	type StoredAuthority,
	type WorkspaceMutationClaimReplace,
	type WorkspaceMutationClaimRollback,
	DecodeReplaceClaim,
	DecodeRollbackClaim,
	DecodeStoredAuthority,
	DecodeStoredJson,
	ValidateSourceAuthority,
	authority_matches_claim,
	conceal_error,
	denied,
	file_scope_contains,
	grant_from_stored,
	invalid_state,
	repository_claim_from,
	rollback_repository_claim_from,
	rollback_source_from,
} from "./model";

export * from "./model";

/** Supplies the SQLite-backed transactional mutation authority. */
export const WorkspaceMutationAuthorityLive = Layer.effect(
	WorkspaceMutationAuthority,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const registry = yield* WorkspaceBoundedRegularFileStoreRegistry;
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
		const ValidatePinnedSourcePath = (authority: StoredAuthority, path: string) =>
			Effect.gen(function* () {
				if (authority.authority_kind !== "graph_run" || authority.scope_kind !== "files") {
					return;
				}

				const controlled_scope = yield* Schema.decodeUnknownEffect(WorkspacePath)(
					authority.scope_value,
				).pipe(Effect.mapError(invalid_state));

				if (!file_scope_contains(controlled_scope, path)) {
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
		/** A source-authority write prevents rollback admission from racing thread erasure. */
		const FenceRollbackSource = (
			transaction: typeof database.client,
			authority: StoredAuthority,
		) =>
			transaction
				.update(WorkspaceMutationAuthorities)
				.set({ created_at: authority.created_at })
				.where(
					and(
						eq(WorkspaceMutationAuthorities.change_id, authority.change_id),
						eq(WorkspaceMutationAuthorities.message_id, authority.message_id),
						eq(WorkspaceMutationAuthorities.thread_id, authority.thread_id),
					),
				)
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
					store: authorized.store,
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
					store: authorized.store,
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
		const ClaimRepositoryRollback = (claim: WorkspaceMutationClaimRollback) =>
			repository
				.ClaimRollback(rollback_repository_claim_from(claim))
				.pipe(
					Effect.mapError((error) =>
						error instanceof JournalStoreFailure ? error.cause : error,
					),
				);
		const ReplaySourceReplace = (claim: WorkspaceMutationClaimReplace) =>
			ClaimRepositoryReplace(claim).pipe(
				Effect.mapError((error) =>
					error instanceof WorkspaceChangeTransitionError ? error : invalid_state(),
				),
				Effect.flatMap((replay) =>
					replay._tag === "duplicate" ? Effect.void : Effect.fail(invalid_state()),
				),
			);
		const InferAuthority = (
			transaction: typeof database.client,
			claim: WorkspaceMutationClaimReplace,
		) =>
			Effect.gen(function* () {
				const [base_run] = yield* transaction
					.select({ run_id: OrchestrationRuns.run_id })
					.from(OrchestrationRuns)
					.where(eq(OrchestrationRuns.run_id, claim.run_id))
					.limit(1);
				const [graph_run] = yield* transaction
					.select({ run_id: AgentRuns.run_id })
					.from(AgentRuns)
					.where(eq(AgentRuns.run_id, claim.run_id))
					.limit(1);

				if (base_run && graph_run) return yield* Effect.fail(invalid_state());
				if (!base_run && !graph_run) return yield* Effect.fail(denied("run_not_active"));

				return yield* base_run
					? ProveBaseRun(transaction, claim)
					: ProveGraphRun(transaction, claim);
			});

		const ClaimReplace = (input: WorkspaceMutationClaimReplace) =>
			DecodeReplaceClaim(input).pipe(
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
										store: authorized.store,
									};
								}

								if (existing_authority) {
									return yield* Effect.fail(
										new WorkspaceMutationAuthorityConflict({
											reason: "authority_conflict",
										}),
									);
								}

								const proof = yield* InferAuthority(transaction, claim);
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
									store: proof.store,
								};
							}),
						),
					),
				),
				Effect.mapError(conceal_error),
			);

		const ClaimRollback = (input: WorkspaceMutationClaimRollback) =>
			DecodeRollbackClaim(input).pipe(
				Effect.flatMap((claim) =>
					RetrySqliteWrite(
						database.client.transaction((transaction) =>
							Effect.gen(function* () {
								const authorities = yield* transaction
									.select()
									.from(WorkspaceMutationAuthorities)
									.where(
										eq(WorkspaceMutationAuthorities.change_id, claim.change_id),
									)
									.limit(2);

								if (authorities.length !== 1) {
									return yield* Effect.fail(invalid_state());
								}

								const authority = yield* DecodeStoredAuthority(authorities[0]);

								if (authority.thread_id !== claim.thread_id) {
									return yield* Effect.fail(
										new WorkspaceMutationAuthorityConflict({
											reason: "authority_conflict",
										}),
									);
								}

								const [source_operation] = yield* transaction
									.select()
									.from(WorkspaceChangeOperations)
									.where(
										eq(
											WorkspaceChangeOperations.message_id,
											authority.message_id,
										),
									)
									.limit(1);
								const [source_change] = yield* transaction
									.select()
									.from(WorkspaceChanges)
									.where(eq(WorkspaceChanges.change_id, authority.change_id))
									.limit(1);

								if (!source_operation || !source_change) {
									return yield* Effect.fail(invalid_state());
								}

								const source_claim = yield* ValidateSourceAuthority(
									authority,
									source_operation,
									source_change,
								);

								yield* ValidatePinnedSourcePath(authority, source_claim.path);

								yield* FenceRollbackSource(transaction, authority);
								yield* ReplaySourceReplace(source_claim);

								const source = rollback_source_from(source_claim);

								const accepted = yield* ClaimRepositoryRollback(claim);
								const source_authority = grant_from_stored(authority);

								if (accepted._tag === "rejected") {
									return {
										_tag: "rejected" as const,
										authority: source_authority,
										claim: accepted,
										source,
									};
								}

								if (accepted._tag === "duplicate") {
									return {
										_tag: "duplicate" as const,
										authority: source_authority,
										claim: accepted,
										source,
									};
								}

								const authorized = yield* AuthorizeWorkspace(
									authority.workspace_id,
									authority.working_directory,
								);

								return {
									_tag: "authorized" as const,
									authority: source_authority,
									claim: accepted,
									source,
									store: authorized.store,
								};
							}),
						),
					),
				),
				Effect.mapError(conceal_error),
			);

		return { ClaimReplace, ClaimRollback };
	}),
);
