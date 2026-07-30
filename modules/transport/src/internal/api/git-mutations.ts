import { Effect } from "effect";

import {
	type GitIndexStageRequestEnvelope,
	type GitIndexUnstageRequestEnvelope,
	type GitMutationResolveEnvelope,
} from "@artisan/protocol";

import type {
	ArtisanCommandReceipt,
	ArtisanGitIndexMutationInput,
	ArtisanGitMutationResolveInput,
} from "../../client-api/service";
import { client_error } from "../client-common";
import { ClientApiContext } from "./context";

type GitMutationEnvelope =
	| GitIndexStageRequestEnvelope
	| GitIndexUnstageRequestEnvelope
	| GitMutationResolveEnvelope;

/** Constructs optimistic Git index mutation operations. */
export const MakeGitMutationApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const send_git_mutation = (envelope: GitMutationEnvelope) =>
		Effect.gen(function* () {
			const result = yield* context.Request(envelope);
			if (result.kind !== "command.receipt") {
				return yield* Effect.die("Git mutation receipt narrowed incorrectly");
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
	const request_git_index_mutation = (input: ArtisanGitIndexMutationInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const message_id = input.command_id ?? trace.message_id;
			const mutation_id =
				input.mutation_id === undefined
					? yield* context.MakeId("git_mutation")
					: input.mutation_id;
			const approval_id =
				input.approval_id === undefined
					? yield* context.MakeId("git_approval")
					: input.approval_id;
			const payload = {
				approval_id,
				expected_snapshot_id: input.expected_snapshot_id,
				expected_workspace_version: input.expected_workspace_version,
				mutation_id,
				paths: input.paths,
				workspace_id: input.workspace_id,
			};
			const attribution = {
				...trace,
				message_id,
				thread_id: input.thread_id,
				...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
				...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
				...(input.run_id === undefined ? {} : { run_id: input.run_id }),
			};
			const envelope: GitIndexStageRequestEnvelope | GitIndexUnstageRequestEnvelope =
				input.kind === "stage"
					? { ...attribution, kind: "git.index.stage.request", payload }
					: { ...attribution, kind: "git.index.unstage.request", payload };
			return yield* send_git_mutation(envelope);
		});
	const resolve_git_mutation = (input: ArtisanGitMutationResolveInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			return yield* send_git_mutation({
				...trace,
				kind: "git.mutation.resolve",
				message_id: input.command_id ?? trace.message_id,
				payload: {
					approval_id: input.approval_id,
					approved: input.approved,
					mutation_id: input.mutation_id,
				},
				thread_id: input.thread_id,
				...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
				...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
				...(input.run_id === undefined ? {} : { run_id: input.run_id }),
			});
		});
	return { request_git_index_mutation, resolve_git_mutation };
});
