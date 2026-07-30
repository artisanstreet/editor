import { Effect } from "effect";

import {
	type GitDiffQueryEnvelope,
	type GitWorkspaceQueryEnvelope,
	type WorkspaceChangeDiffQueryEnvelope,
	type WorkspaceChangeListQueryEnvelope,
	type WorkspaceChangeReviewEnvelope,
	type WorkspaceChangeRollbackEnvelope,
	type WorkspaceConflictListQueryEnvelope,
	type WorkspaceFileDiscoveryQueryEnvelope,
	type WorkspaceFileReadQueryEnvelope,
	type WorkspaceFileReplaceEnvelope,
	type WorkspaceLanguageCapabilitiesQueryEnvelope,
} from "@artisan/protocol";

import type {
	ArtisanCommandReceipt,
	ArtisanGitDiffInput,
	ArtisanGitWorkspaceInput,
	ArtisanWorkspaceChangeDiffInput,
	ArtisanWorkspaceChangeListInput,
	ArtisanWorkspaceChangeReviewInput,
	ArtisanWorkspaceChangeRollbackInput,
	ArtisanWorkspaceFileDiscoveryInput,
	ArtisanWorkspaceFileReadInput,
	ArtisanWorkspaceFileReplaceInput,
	ArtisanWorkspaceLanguageCapabilitiesInput,
} from "../../client-api/service";
import { client_error } from "../client-common";
import { ClientApiContext } from "./context";

type WorkspaceMutationEnvelope =
	| WorkspaceChangeReviewEnvelope
	| WorkspaceChangeRollbackEnvelope
	| WorkspaceFileReplaceEnvelope;

/** Constructs workspace read and mutation operations. */
export const MakeWorkspaceApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const read_workspace_file = (input: ArtisanWorkspaceFileReadInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: WorkspaceFileReadQueryEnvelope = {
				...trace,
				kind: "workspace.file.read.query",
				payload: input,
			};
			return (yield* context.Request(envelope)).payload;
		});
	const list_workspace_files = (input: ArtisanWorkspaceFileDiscoveryInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: WorkspaceFileDiscoveryQueryEnvelope = {
				...trace,
				kind: "workspace.file.discovery.query",
				payload: input,
			};
			return (yield* context.Request(envelope)).payload;
		});
	const get_workspace_language_capabilities = (
		input: ArtisanWorkspaceLanguageCapabilitiesInput,
	) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: WorkspaceLanguageCapabilitiesQueryEnvelope = {
				...trace,
				kind: "workspace.language.capabilities.query",
				payload: input,
			};
			return (yield* context.Request(envelope)).payload;
		});
	const list_workspace_changes = (input: ArtisanWorkspaceChangeListInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: WorkspaceChangeListQueryEnvelope = {
				...trace,
				kind: "workspace.change.list.query",
				payload: input,
			};
			return (yield* context.Request(envelope)).payload;
		});
	const get_workspace_change_diff = (input: ArtisanWorkspaceChangeDiffInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: WorkspaceChangeDiffQueryEnvelope = {
				...trace,
				kind: "workspace.change.diff.query",
				payload: input,
			};
			return (yield* context.Request(envelope)).payload;
		});
	const list_workspace_conflicts = (thread_id: string) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: WorkspaceConflictListQueryEnvelope = {
				...trace,
				kind: "workspace.conflict.list.query",
				payload: { thread_id },
			};
			return (yield* context.Request(envelope)).payload;
		});
	const get_git_workspace = (input: ArtisanGitWorkspaceInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: GitWorkspaceQueryEnvelope = {
				...trace,
				kind: "git.workspace.query",
				payload: input,
			};
			return (yield* context.Request(envelope)).payload;
		});
	const get_git_diff = (input: ArtisanGitDiffInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: GitDiffQueryEnvelope = {
				...trace,
				kind: "git.diff.query",
				payload: input,
			};
			return (yield* context.Request(envelope)).payload;
		});
	const send_workspace_mutation = (envelope: WorkspaceMutationEnvelope) =>
		Effect.gen(function* () {
			const result = yield* context.Request(envelope);
			if (result.kind !== "command.receipt") {
				return yield* Effect.die("workspace mutation receipt narrowed incorrectly");
			}
			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}
			return {
				command_id: envelope.message_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});
	const replace_workspace_file = (input: ArtisanWorkspaceFileReplaceInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send_workspace_mutation({
				...trace,
				agent_id: input.agent_id,
				kind: "workspace.file.replace",
				message_id: input.command_id ?? trace.message_id,
				payload: {
					change_id: input.change_id,
					content: input.content,
					expected_before: input.expected_before,
					path: input.path,
					workspace_id: input.workspace_id,
				},
				run_id: input.run_id,
				thread_id: input.thread_id,
				...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
			} satisfies WorkspaceFileReplaceEnvelope);
		});
	const review_workspace_change = (input: ArtisanWorkspaceChangeReviewInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send_workspace_mutation({
				...trace,
				kind: "workspace.change.review",
				message_id: input.command_id ?? trace.message_id,
				payload: {
					change_id: input.change_id,
					reviewer_kind: input.reviewer_kind,
					...(input.comment === undefined ? {} : { comment: input.comment }),
					...(input.outcome === undefined ? {} : { outcome: input.outcome }),
					...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
					...(input.reviewer_kind === "user"
						? {}
						: {
								assignment_id: input.assignment_id,
								group_id: input.group_id,
								reviewer_agent_id: input.reviewer_agent_id,
								reviewer_run_id: input.reviewer_run_id,
							}),
				},
				thread_id: input.thread_id,
			} satisfies WorkspaceChangeReviewEnvelope);
		});
	const rollback_workspace_change = (input: ArtisanWorkspaceChangeRollbackInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send_workspace_mutation({
				...trace,
				kind: "workspace.change.rollback",
				message_id: input.command_id ?? trace.message_id,
				payload: {
					change_id: input.change_id,
					expected_after: input.expected_after,
				},
				thread_id: input.thread_id,
			} satisfies WorkspaceChangeRollbackEnvelope);
		});

	return {
		get_git_diff,
		get_git_workspace,
		get_workspace_change_diff,
		get_workspace_language_capabilities,
		list_workspace_changes,
		list_workspace_conflicts,
		list_workspace_files,
		read_workspace_file,
		replace_workspace_file,
		review_workspace_change,
		rollback_workspace_change,
	};
});
