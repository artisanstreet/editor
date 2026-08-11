import { and, asc, eq, isNotNull, isNull } from "drizzle-orm";
import { Effect, Schema } from "effect";

import {
	EngineSubagentTranscriptContent,
	type EngineObservation,
	type EngineSubagentTranscriptContent as SubagentTranscriptContent,
} from "@artisan/engines";

import { ApplyEngineObservation } from "../../conversation/index";
import {
	ConversationSources,
	NativeSubagentBindings,
	NativeSubagentTranscriptInbox,
	OrchestrationRuns,
} from "../../persistence/tables";
import { normalize_graph_error } from "../agent-graph-model";
import type { GraphContext } from "./graph-context";

type PendingTranscript = typeof NativeSubagentTranscriptInbox.$inferSelect;

const DecodeContent = (value: string) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(EngineSubagentTranscriptContent)),
	);

const ObservationFor = (
	row: PendingTranscript,
	content: SubagentTranscriptContent,
	run_id: string,
): EngineObservation => {
	const base = {
		artisan_run_id: run_id,
		native_thread_id: row.agent_native_thread_id,
		observation_id: row.observation_id,
		raw: { engine_id: row.engine_id, frame: content, transport: "native-subagent-replay" },
		sequence: row.sequence,
	};
	const turn_id = `native:${row.agent_native_thread_id}`;
	switch (content._tag) {
		case "agent_message_delta":
		case "agent_message_completed":
		case "reasoning_summary_delta":
		case "reasoning_summary_completed":
			return { ...base, ...content, turn_id };
		case "terminal_activity":
			return {
				...base,
				_tag: content._tag,
				activity_id: content.activity_id,
				...(content.channel === undefined ? {} : { channel: content.channel }),
				...(content.command === undefined ? {} : { command: content.command }),
				...(content.exit_code === undefined ? {} : { exit_code: content.exit_code }),
				...(content.output === undefined ? {} : { output: content.output }),
				state: content.state,
			};
		case "tool":
			return {
				...base,
				_tag: content._tag,
				action: content.action,
				...(content.detail === undefined ? {} : { detail: content.detail }),
				tool_id: content.tool_id,
				tool_name: content.tool_name,
			};
		case "file":
			return {
				...base,
				_tag: content._tag,
				action: content.action,
				path: content.path,
				...(content.lines_added === undefined ? {} : { lines_added: content.lines_added }),
				...(content.lines_deleted === undefined
					? {}
					: { lines_deleted: content.lines_deleted }),
			};
		case "search":
			return {
				...base,
				_tag: content._tag,
				query: content.query,
				state: content.state,
				...(content.result_count === undefined
					? {}
					: { result_count: content.result_count }),
				...(content.scope === undefined ? {} : { scope: content.scope }),
				...(content.search_id === undefined ? {} : { search_id: content.search_id }),
			};
	}
};

/** Replays child-only canonical content once its durable lifecycle binding exists. */
export const make_native_subagent_transcripts = (context: GraphContext) => {
	const RecordPending = (observation_id: string) =>
		context.database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [pending] = yield* transaction
						.select()
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
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, pending.root_run_id))
						.limit(1);
					const [binding] = yield* transaction
						.select()
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
					if (root === undefined || binding === undefined) return;
					const content = yield* DecodeContent(pending.content_json);
					const occurred_at = yield* context.metadata.Now;
					yield* ApplyEngineObservation(
						transaction,
						ObservationFor(pending, content, binding.run_id),
						{
							agent_id: binding.agent_id,
							occurred_at,
							parent_run_id: root.run_id,
							run_id: binding.run_id,
							thread_id: root.thread_id,
						},
					);
					yield* transaction
						.delete(ConversationSources)
						.where(
							and(
								eq(
									ConversationSources.source_id,
									`observation:${pending.observation_id}`,
								),
								eq(ConversationSources.thread_id, root.thread_id),
							),
						);
					yield* transaction
						.delete(NativeSubagentTranscriptInbox)
						.where(
							and(
								eq(
									NativeSubagentTranscriptInbox.observation_id,
									pending.observation_id,
								),
								isNull(NativeSubagentTranscriptInbox.processed_at),
							),
						);
				}),
			)
			.pipe(Effect.mapError(normalize_graph_error));
	const DrainRoot = (root_run_id: string) =>
		Effect.gen(function* () {
			const pending = yield* context.database.client
				.select({ observation_id: NativeSubagentTranscriptInbox.observation_id })
				.from(NativeSubagentTranscriptInbox)
				.where(
					and(
						eq(NativeSubagentTranscriptInbox.root_run_id, root_run_id),
						isNull(NativeSubagentTranscriptInbox.processed_at),
					),
				)
				.orderBy(
					asc(NativeSubagentTranscriptInbox.sequence),
					asc(NativeSubagentTranscriptInbox.observation_id),
				);
			yield* Effect.forEach(pending, (row) => RecordPending(row.observation_id), {
				discard: true,
			});
		});
	const Recover = Effect.gen(function* () {
		/** A prior build marked successful replays but retained their full payloads forever. */
		yield* context.database.client
			.delete(NativeSubagentTranscriptInbox)
			.where(isNotNull(NativeSubagentTranscriptInbox.processed_at));
		const pending = yield* context.database.client
			.selectDistinct({ root_run_id: NativeSubagentTranscriptInbox.root_run_id })
			.from(NativeSubagentTranscriptInbox)
			.where(isNull(NativeSubagentTranscriptInbox.processed_at));
		yield* Effect.forEach(pending, (row) => DrainRoot(row.root_run_id), { discard: true });
	}).pipe(Effect.mapError(normalize_graph_error));
	return { DrainRoot, RecordPending, Recover };
};
