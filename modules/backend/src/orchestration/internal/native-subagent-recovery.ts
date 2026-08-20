import { asc, eq, isNull } from "drizzle-orm";
import { Effect } from "effect";

import {
	Assignments,
	NativeSubagentBindings,
	NativeSubagentObservationInbox,
	NativeSubagentTranscriptInbox,
	OrchestrationRuns,
} from "../../persistence/tables";
import type { GraphContext } from "./graph-context";
import { friendly_native_role } from "./graph-context";

const recovery_root_concurrency = 4;

const GroupByRoot = <Entry extends { readonly root_run_id: string }>(
	entries: ReadonlyArray<Entry>,
) => {
	const groups = new Map<string, Array<Entry>>();

	for (const entry of entries) {
		const existing = groups.get(entry.root_run_id);
		if (existing !== undefined) {
			existing.push(entry);
		} else {
			groups.set(entry.root_run_id, [entry]);
		}
	}

	return [...groups.values()];
};

const GroupByThread = <
	Entry extends { readonly root_run_id: string; readonly thread_id: string | null },
>(
	entries: ReadonlyArray<Entry>,
) => {
	const groups = new Map<string, Array<Entry>>();
	for (const entry of entries) {
		const key = entry.thread_id ?? `missing-root:${entry.root_run_id}`;
		const group = groups.get(key);
		if (group) group.push(entry);
		else groups.set(key, [entry]);
	}
	return [...groups.values()];
};

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
		const persisted_bindings = yield* context.database.client
			.select({
				agent_path: NativeSubagentBindings.agent_path,
				assignment_id: NativeSubagentBindings.assignment_id,
				assignment_role: Assignments.role,
				root_run_id: NativeSubagentBindings.root_run_id,
				thread_id: OrchestrationRuns.thread_id,
			})
			.from(NativeSubagentBindings)
			.leftJoin(
				Assignments,
				eq(NativeSubagentBindings.assignment_id, Assignments.assignment_id),
			)
			.leftJoin(
				OrchestrationRuns,
				eq(NativeSubagentBindings.root_run_id, OrchestrationRuns.run_id),
			)
			.orderBy(asc(NativeSubagentBindings.root_run_id));
		yield* Effect.forEach(
			persisted_bindings.filter(
				(binding) =>
					binding.assignment_role !== null &&
					binding.assignment_role !== friendly_native_role(binding.agent_path),
			),
			(binding) =>
				context.database.client
					.update(Assignments)
					.set({ role: friendly_native_role(binding.agent_path) })
					.where(eq(Assignments.assignment_id, binding.assignment_id)),
			{ concurrency: recovery_root_concurrency, discard: true },
		);
		const pending = yield* context.database.client
			.select({
				observation_id: NativeSubagentObservationInbox.observation_id,
				root_run_id: NativeSubagentObservationInbox.root_run_id,
				thread_id: OrchestrationRuns.thread_id,
			})
			.from(NativeSubagentObservationInbox)
			.leftJoin(
				OrchestrationRuns,
				eq(NativeSubagentObservationInbox.root_run_id, OrchestrationRuns.run_id),
			)
			.where(isNull(NativeSubagentObservationInbox.processed_at))
			.orderBy(
				asc(NativeSubagentObservationInbox.root_run_id),
				asc(NativeSubagentObservationInbox.sequence),
				asc(NativeSubagentObservationInbox.observation_id),
			);
		const pending_root_ids = new Set(pending.map((observation) => observation.root_run_id));
		yield* Effect.forEach(
			GroupByThread(pending),
			(thread_entries) =>
				Effect.forEach(
					GroupByRoot(thread_entries),
					(observations) =>
						Effect.gen(function* () {
							/**
							 * Reconcile after every provider-sequenced observation for this root:
							 * collapsing its replay would erase observable intermediate aggregate
							 * lifecycle events. Independent roots have no shared provider sequence.
							 */
							yield* Effect.forEach(
								observations,
								(observation) =>
									input
										.RecordPending(observation.observation_id, name_bank)
										.pipe(
											Effect.andThen(
												input.ReconcileRoot(observation.root_run_id),
											),
										),
								{ concurrency: 1, discard: true },
							);
						}),
					{ concurrency: 1, discard: true },
				),
			{ concurrency: recovery_root_concurrency, discard: true },
		);
		const pending_transcripts = yield* context.database.client
			.select({
				observation_id: NativeSubagentTranscriptInbox.observation_id,
				root_run_id: NativeSubagentTranscriptInbox.root_run_id,
				thread_id: OrchestrationRuns.thread_id,
			})
			.from(NativeSubagentTranscriptInbox)
			.leftJoin(
				OrchestrationRuns,
				eq(NativeSubagentTranscriptInbox.root_run_id, OrchestrationRuns.run_id),
			)
			.where(isNull(NativeSubagentTranscriptInbox.processed_at))
			.orderBy(
				asc(NativeSubagentTranscriptInbox.root_run_id),
				asc(NativeSubagentTranscriptInbox.sequence),
				asc(NativeSubagentTranscriptInbox.observation_id),
			);
		yield* Effect.forEach(
			GroupByThread(pending_transcripts),
			(thread_entries) =>
				Effect.forEach(
					GroupByRoot(thread_entries),
					(observations) =>
						Effect.forEach(
							observations,
							(observation) =>
								input.ConsumeTerminalTranscript(observation.observation_id),
							{ concurrency: 1, discard: true },
						),
					{ concurrency: 1, discard: true },
				),
			{ concurrency: recovery_root_concurrency, discard: true },
		);
		yield* input.RecoverTranscripts;

		yield* Effect.forEach(
			GroupByThread(
				persisted_bindings.filter((binding) => !pending_root_ids.has(binding.root_run_id)),
			),
			(thread_entries) =>
				Effect.forEach(
					[...new Set(thread_entries.map((entry) => entry.root_run_id))],
					(root_run_id) => input.ReconcileRoot(root_run_id),
					{ concurrency: 1, discard: true },
				),
			{ concurrency: recovery_root_concurrency, discard: true },
		);
	});
