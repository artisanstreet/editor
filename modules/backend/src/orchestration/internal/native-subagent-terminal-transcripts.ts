import { and, eq, isNull } from "drizzle-orm";
import { Effect } from "effect";

import {
	NativeSubagentBindings,
	NativeSubagentTranscriptInbox,
	OrchestrationRuns,
} from "../../persistence/tables";
import type { GraphContext } from "./graph-context";

/** Consumes transcript frames that arrived after the root provider session settled. */
export const MakeTerminalTranscriptConsumption = (context: GraphContext) => {
	const Consume = (observation_id: string) =>
		context.database.client.transaction((transaction) =>
			Effect.gen(function* () {
				const [pending] = yield* transaction
					.select({
						agent_native_thread_id:
							NativeSubagentTranscriptInbox.agent_native_thread_id,
						engine_id: NativeSubagentTranscriptInbox.engine_id,
						root_run_id: NativeSubagentTranscriptInbox.root_run_id,
					})
					.from(NativeSubagentTranscriptInbox)
					.where(
						and(
							eq(NativeSubagentTranscriptInbox.observation_id, observation_id),
							isNull(NativeSubagentTranscriptInbox.processed_at),
						),
					)
					.limit(1);
				if (pending === undefined) return;
				const [root] = yield* transaction
					.select({ status: OrchestrationRuns.status })
					.from(OrchestrationRuns)
					.where(eq(OrchestrationRuns.run_id, pending.root_run_id))
					.limit(1);
				if (root === undefined || ["queued", "running", "waiting"].includes(root.status)) {
					return;
				}
				const [binding] = yield* transaction
					.select({ binding_id: NativeSubagentBindings.binding_id })
					.from(NativeSubagentBindings)
					.where(
						and(
							eq(NativeSubagentBindings.engine_id, pending.engine_id),
							eq(NativeSubagentBindings.root_run_id, pending.root_run_id),
							eq(
								NativeSubagentBindings.agent_native_thread_id,
								pending.agent_native_thread_id,
							),
						),
					)
					.limit(1);
				/**
				 * A root can settle before its provider flushes a known child's final
				 * frames. The durable binding makes those frames unambiguous, so leave
				 * them for normal child projection instead of treating them as root noise.
				 */
				if (binding !== undefined) return;
				yield* transaction
					.delete(NativeSubagentTranscriptInbox)
					.where(
						and(
							eq(NativeSubagentTranscriptInbox.observation_id, observation_id),
							isNull(NativeSubagentTranscriptInbox.processed_at),
						),
					);
			}),
		);

	return { Consume };
};
