import { and, eq, isNull } from "drizzle-orm";
import { Effect } from "effect";

import { NativeSubagentTranscriptInbox, OrchestrationRuns } from "../../persistence/tables";
import type { GraphContext } from "./graph-context";

/** Consumes transcript frames that arrived after the root provider session settled. */
export const MakeTerminalTranscriptConsumption = (context: GraphContext) => {
	const Consume = (observation_id: string) =>
		context.database.client.transaction((transaction) =>
			Effect.gen(function* () {
				const [pending] = yield* transaction
					.select({ root_run_id: NativeSubagentTranscriptInbox.root_run_id })
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
