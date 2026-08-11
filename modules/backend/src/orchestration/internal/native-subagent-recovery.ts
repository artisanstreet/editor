import { asc, isNull } from "drizzle-orm";
import { Effect } from "effect";

import {
	NativeSubagentBindings,
	NativeSubagentObservationInbox,
	NativeSubagentTranscriptInbox,
} from "../../persistence/tables";
import type { GraphContext } from "./graph-context";

/** Recovers native inboxes in provider sequence before resuming child transcript projection. */
export const RecoverNativeSubagents = (
	context: GraphContext,
	input: {
		readonly ConsumeTerminalTranscript: (
			observation_id: string,
		) => Effect.Effect<void, unknown>;
		readonly ReconcileRoot: (root_run_id: string) => Effect.Effect<void, unknown>;
		readonly RecordPending: (
			observation_id: string,
			name_bank: ReadonlyArray<string>,
		) => Effect.Effect<unknown, unknown>;
		readonly RecoverTranscripts: Effect.Effect<void, unknown>;
	},
) =>
	Effect.gen(function* () {
		const name_bank = yield* context.agent_name_catalog.Names;
		const pending = yield* context.database.client
			.select({
				observation_id: NativeSubagentObservationInbox.observation_id,
				root_run_id: NativeSubagentObservationInbox.root_run_id,
			})
			.from(NativeSubagentObservationInbox)
			.where(isNull(NativeSubagentObservationInbox.processed_at))
			.orderBy(
				asc(NativeSubagentObservationInbox.root_run_id),
				asc(NativeSubagentObservationInbox.sequence),
				asc(NativeSubagentObservationInbox.observation_id),
			);
		yield* Effect.forEach(
			pending,
			(observation) =>
				input
					.RecordPending(observation.observation_id, name_bank)
					.pipe(Effect.andThen(input.ReconcileRoot(observation.root_run_id))),
			{ discard: true },
		);
		const pending_transcripts = yield* context.database.client
			.select({ observation_id: NativeSubagentTranscriptInbox.observation_id })
			.from(NativeSubagentTranscriptInbox)
			.where(isNull(NativeSubagentTranscriptInbox.processed_at))
			.orderBy(
				asc(NativeSubagentTranscriptInbox.root_run_id),
				asc(NativeSubagentTranscriptInbox.sequence),
				asc(NativeSubagentTranscriptInbox.observation_id),
			);
		yield* Effect.forEach(
			pending_transcripts,
			(observation) => input.ConsumeTerminalTranscript(observation.observation_id),
			{ discard: true },
		);
		yield* input.RecoverTranscripts;

		const roots = yield* context.database.client
			.selectDistinct({ root_run_id: NativeSubagentBindings.root_run_id })
			.from(NativeSubagentBindings);
		yield* Effect.forEach(roots, (root) => input.ReconcileRoot(root.root_run_id), {
			discard: true,
		});
	});
