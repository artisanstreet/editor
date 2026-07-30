import { Effect } from "effect";

import type { ArtisanApprovalResolveEnvelope, ArtisanToolExecuteEnvelope } from "@artisan/protocol";

import { JournalStore } from "../../../persistence/journal-store";
import { RuntimeMetadata } from "../../../runtime/metadata";
import { ToolControlPlane } from "../../../tools/tool-control-plane";
import type { ReadyState } from "../../connection-state";
import { ConnectionResponseSink } from "../query-handlers/project";

export const MakeToolMutationHandlers = Effect.gen(function* () {
	const journal = yield* JournalStore;
	const metadata = yield* RuntimeMetadata;
	const tools = yield* ToolControlPlane;
	const { Enqueue, EnqueueError } = yield* ConnectionResponseSink;

	const HandleToolExecute = (query: ArtisanToolExecuteEnvelope, current: ReadyState) =>
		tools
			.Execute({
				...(query.agent_id === undefined ? {} : { agent_id: query.agent_id }),
				request: query.payload,
				...(query.run_id === undefined ? {} : { run_id: query.run_id }),
				thread_id: query.thread_id,
			})
			.pipe(
				Effect.flatMap(() =>
					Effect.gen(function* () {
						yield* Enqueue({
							causation_id: query.message_id,
							correlation_id: query.message_id,
							kind: "command.receipt",
							message_id: yield* metadata.MakeId("message"),
							origin: "backend",
							payload: {
								journal_sequence: yield* journal.ReadWatermark(),
								status: "accepted",
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: yield* metadata.Now,
							thread_id: query.thread_id,
						});
					}),
				),
				Effect.catch(() =>
					EnqueueError(
						current,
						"artisan.tool.execution_failed",
						"The tool invocation could not be routed.",
						true,
						query.message_id,
					),
				),
			);
	const HandleToolApprovalResolve = (
		query: ArtisanApprovalResolveEnvelope,
		current: ReadyState,
	) =>
		tools.ResolveApproval({ request: query.payload, thread_id: query.thread_id }).pipe(
			Effect.flatMap(() =>
				Effect.gen(function* () {
					yield* Enqueue({
						causation_id: query.message_id,
						correlation_id: query.message_id,
						kind: "command.receipt",
						message_id: yield* metadata.MakeId("message"),
						origin: "backend",
						payload: {
							journal_sequence: yield* journal.ReadWatermark(),
							status: "accepted",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: yield* metadata.Now,
						thread_id: query.thread_id,
					});
				}),
			),
			Effect.catch(() =>
				EnqueueError(
					current,
					"artisan.approval.resolve_failed",
					"The approval could not be resolved.",
					true,
					query.message_id,
				),
			),
		);
	return { HandleToolApprovalResolve, HandleToolExecute };
});
