import { Effect, Schema } from "effect";
import { and, asc, eq, gt, sql } from "drizzle-orm";

import type { EngineObservation } from "@artisan/engines";
import {
	ConversationItem,
	ConversationPatch,
	ConversationSnapshot,
	ConversationTurn,
	type EventEnvelope,
} from "@artisan/protocol";

import {
	ConversationItems,
	ConversationPatches,
	ConversationSources,
	ConversationThreads,
	ConversationTurns,
} from "../persistence/schema";

export class ConversationProjectionError extends Error {
	readonly _tag = "ConversationProjectionError";
}

/** Caps every replay query and its corresponding transport envelope. */
export const conversation_patch_replay_batch_size = 64;

export interface ConversationObservationContext {
	readonly agent_id?: string;
	readonly occurred_at: string;
	readonly run_id: string;
	readonly thread_id: string;
}

const Decode = (schema: any, value: unknown, context: string) =>
	Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value).pipe(
		Effect.mapError(() => new ConversationProjectionError(`Invalid ${context}`)),
	);

const lifecycle = (state: string) =>
	state === "completed" || state === "closed"
		? "completed"
		: state === "cancelled"
			? "cancelled"
			: state === "failed" || state === "interrupted"
				? "failed"
				: state === "waiting"
					? "waiting"
					: state === "pending"
						? "pending"
						: state === "streaming"
							? "streaming"
							: "active";

const source_refs = (
	reference: string,
	input: { event_id?: string; journal_sequence?: number; provider?: string },
) => [
	{
		reference,
		...(input.event_id === undefined ? {} : { event_id: input.event_id }),
		...(input.journal_sequence === undefined
			? {}
			: { journal_sequence: input.journal_sequence }),
		...(input.provider === undefined ? {} : { provider: input.provider }),
	},
];

const text = (value: string) => value.slice(0, 4_096);

const EnsureThread = (transaction: any, thread_id: string, updated_at: string) =>
	transaction
		.insert(ConversationThreads)
		.values({
			thread_id,
			updated_at,
			next_ordinal: 0,
			last_patch_sequence: 0,
			journal_sequence: 0,
		})
		.onConflictDoNothing();

const AllocateOrdinal = (transaction: any, thread_id: string) =>
	Effect.gen(function* () {
		const [allocated] = yield* transaction
			.update(ConversationThreads)
			.set({ next_ordinal: sql`${ConversationThreads.next_ordinal} + 1` })
			.where(eq(ConversationThreads.thread_id, thread_id))
			.returning({ next_ordinal: ConversationThreads.next_ordinal });
		if (!allocated)
			return yield* Effect.fail(
				new ConversationProjectionError("Conversation thread is missing"),
			);
		return allocated.next_ordinal - 1;
	});

const Emit = (
	transaction: any,
	thread_id: string,
	updated_at: string,
	patch: any,
	journal_sequence?: number,
) =>
	Effect.gen(function* () {
		const [allocated] = yield* transaction
			.update(ConversationThreads)
			.set({
				last_patch_sequence: sql`${ConversationThreads.last_patch_sequence} + 1`,
				...(journal_sequence === undefined
					? {}
					: {
							journal_sequence: sql`max(${ConversationThreads.journal_sequence}, ${journal_sequence})`,
						}),
				updated_at,
			})
			.where(eq(ConversationThreads.thread_id, thread_id))
			.returning({ sequence: ConversationThreads.last_patch_sequence });
		if (!allocated)
			return yield* Effect.fail(
				new ConversationProjectionError("Conversation thread is missing"),
			);
		const sequence = allocated.sequence;
		const decoded = yield* Decode(
			ConversationPatch,
			{ ...patch, patch_id: `conversation:patch:${thread_id}:${sequence}`, sequence },
			"conversation patch",
		);
		yield* transaction.insert(ConversationPatches).values({
			patch_id: decoded.patch_id,
			thread_id,
			sequence,
			patch_json: JSON.stringify(decoded),
		});
	});

const Admit = (
	transaction: any,
	source_id: string,
	thread_id: string,
	observed_at: string,
	journal_sequence?: number,
) =>
	transaction
		.insert(ConversationSources)
		.values({ source_id, thread_id, observed_at, journal_sequence: journal_sequence ?? null })
		.onConflictDoNothing()
		.returning({ source_id: ConversationSources.source_id })
		.pipe(Effect.map((rows: ReadonlyArray<unknown>) => rows.length > 0));

const UpsertTurn = (
	transaction: any,
	thread_id: string,
	turn: any,
	source: { observed_at: string; journal_sequence?: number },
) =>
	Effect.gen(function* () {
		const [existing] = yield* transaction
			.select()
			.from(ConversationTurns)
			.where(eq(ConversationTurns.turn_id, turn.id))
			.limit(1);
		if (!existing) {
			const ordinal = yield* AllocateOrdinal(transaction, thread_id);
			const entity = yield* Decode(
				ConversationTurn,
				{ ...turn, ordinal, revision: 0 },
				"conversation turn",
			);
			yield* transaction.insert(ConversationTurns).values({
				turn_id: entity.id,
				thread_id,
				ordinal,
				entity_json: JSON.stringify(entity),
			});
			yield* Emit(
				transaction,
				thread_id,
				source.observed_at,
				{ type: "turn_upsert", turn: entity },
				source.journal_sequence,
			);
			return entity;
		}
		const prior = yield* Decode(
			ConversationTurn,
			JSON.parse(existing.entity_json),
			"stored conversation turn",
		);
		if (
			prior.lifecycle === "completed" ||
			prior.lifecycle === "failed" ||
			prior.lifecycle === "cancelled"
		)
			return prior;
		/** Streaming observations do not change the stable active turn itself. */
		if (
			prior.lifecycle === lifecycle(turn.lifecycle) &&
			prior.run_id === turn.run_id &&
			prior.agent_id === turn.agent_id
		)
			return prior;
		const entity = yield* Decode(
			ConversationTurn,
			{ ...prior, ...turn, ordinal: prior.ordinal, revision: prior.revision + 1 },
			"conversation turn",
		);
		yield* transaction
			.update(ConversationTurns)
			.set({ entity_json: JSON.stringify(entity) })
			.where(eq(ConversationTurns.turn_id, entity.id));
		yield* Emit(
			transaction,
			thread_id,
			source.observed_at,
			{ type: "turn_upsert", turn: entity },
			source.journal_sequence,
		);
		return entity;
	});

const UpsertItem = (
	transaction: any,
	thread_id: string,
	item: any,
	source: { observed_at: string; journal_sequence?: number },
) =>
	Effect.gen(function* () {
		const [existing] = yield* transaction
			.select()
			.from(ConversationItems)
			.where(eq(ConversationItems.item_id, item.id))
			.limit(1);
		if (!existing) {
			const ordinal = yield* AllocateOrdinal(transaction, thread_id);
			const entity = yield* Decode(
				ConversationItem,
				{ ...item, ordinal, revision: 0 },
				"conversation item",
			);
			yield* transaction.insert(ConversationItems).values({
				item_id: entity.id,
				thread_id,
				turn_id: entity.turn_id,
				ordinal,
				entity_json: JSON.stringify(entity),
			});
			yield* Emit(
				transaction,
				thread_id,
				source.observed_at,
				{ type: "item_upsert", item: entity },
				source.journal_sequence,
			);
			return entity;
		}
		const prior = yield* Decode(
			ConversationItem,
			JSON.parse(existing.entity_json),
			"stored conversation item",
		);
		if (["completed", "failed", "cancelled"].includes(prior.lifecycle)) return prior;
		const entity = yield* Decode(
			ConversationItem,
			{
				...prior,
				...item,
				...(prior.type === "work_session" && item.type === "work_session"
					? { started_at: prior.started_at }
					: {}),
				ordinal: prior.ordinal,
				turn_id: prior.turn_id,
				revision: prior.revision + 1,
			},
			"conversation item",
		);
		yield* transaction
			.update(ConversationItems)
			.set({ entity_json: JSON.stringify(entity) })
			.where(eq(ConversationItems.item_id, entity.id));
		yield* Emit(
			transaction,
			thread_id,
			source.observed_at,
			{ type: "item_upsert", item: entity },
			source.journal_sequence,
		);
		return entity;
	});

/** Appends a provider delta to its stable item while retaining one renderer entity. */
const AppendText = (
	transaction: any,
	thread_id: string,
	item_id: string,
	turn_id: string,
	input: ConversationObservationContext,
	value: string,
	type: "assistant_message" | "reasoning_summary",
	source: { observed_at: string },
	phase?: "commentary" | "final" | "unspecified",
) =>
	Effect.gen(function* () {
		const [existing] = yield* transaction
			.select()
			.from(ConversationItems)
			.where(eq(ConversationItems.item_id, item_id))
			.limit(1);
		if (!existing)
			return yield* UpsertItem(
				transaction,
				thread_id,
				{
					...item_base(item_id, turn_id, input, "streaming", item_id),
					type,
					text: text(value),
					...(type === "assistant_message" ? { phase: phase ?? "unspecified" } : {}),
				},
				source,
			);
		const prior = yield* Decode(
			ConversationItem,
			JSON.parse(existing.entity_json),
			"stored conversation item",
		);
		if (
			(prior.type !== "assistant_message" && prior.type !== "reasoning_summary") ||
			prior.lifecycle !== "streaming"
		)
			return prior;
		const delta = text(value).slice(0, Math.max(0, 4_096 - prior.text.length));
		const revision = prior.revision + 1;
		const entity = {
			...prior,
			text: prior.text + delta,
			revision,
			updated_at: input.occurred_at,
		};
		yield* transaction
			.update(ConversationItems)
			.set({ entity_json: JSON.stringify(entity) })
			.where(eq(ConversationItems.item_id, item_id));
		yield* Emit(transaction, thread_id, input.occurred_at, {
			type: "item_append",
			item_id,
			text: delta,
			revision,
		});
		return entity;
	});

const turn_base = (
	id: string,
	input: ConversationObservationContext,
	state = "active",
	reference = id,
) => ({
	id,
	type: "turn" as const,
	created_at: input.occurred_at,
	updated_at: input.occurred_at,
	lifecycle: lifecycle(state),
	references: [],
	source_refs: source_refs(reference, { provider: "engine" }),
	...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
	run_id: input.run_id,
});

const item_base = (
	id: string,
	turn_id: string,
	input: ConversationObservationContext,
	state = "active",
	reference = id,
) => ({
	id,
	turn_id,
	created_at: input.occurred_at,
	updated_at: input.occurred_at,
	lifecycle: lifecycle(state),
	references: [],
	source_refs: source_refs(reference, { provider: "engine" }),
	...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
	run_id: input.run_id,
});

/** Applies one raw-engine-normalized observation in the caller's transaction. */
export const ApplyEngineObservation = (
	transaction: any,
	observation: EngineObservation,
	input: ConversationObservationContext,
) =>
	Effect.gen(function* () {
		yield* EnsureThread(transaction, input.thread_id, input.occurred_at);
		const admitted = yield* Admit(
			transaction,
			`observation:${observation.observation_id}`,
			input.thread_id,
			input.occurred_at,
		);
		if (!admitted) return;
		/**
		 * One Artisan run is one renderer turn. Provider-native turn identifiers remain
		 * available in raw provenance, but must not create a second visible work session
		 * for the same prompt.
		 */
		const turn_id = `run:${input.run_id}`;
		yield* UpsertTurn(
			transaction,
			input.thread_id,
			turn_base(turn_id, input, "active", observation.observation_id),
			{ observed_at: input.occurred_at },
		);
		const common = { observed_at: input.occurred_at };
		switch (observation._tag) {
			case "agent_message_delta":
				return yield* AppendText(
					transaction,
					input.thread_id,
					observation.item_id,
					turn_id,
					input,
					observation.delta,
					"assistant_message",
					common,
					observation.phase,
				);
			case "agent_message_completed":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							observation.item_id,
							turn_id,
							input,
							"completed",
							observation.observation_id,
						),
						type: "assistant_message",
						text: text(observation.message),
						phase: observation.phase,
					},
					common,
				);
			case "reasoning_summary_delta":
				return yield* AppendText(
					transaction,
					input.thread_id,
					observation.item_id,
					turn_id,
					input,
					observation.delta,
					"reasoning_summary",
					common,
				);
			case "turn_state":
				return yield* Effect.gen(function* () {
					yield* UpsertTurn(
						transaction,
						input.thread_id,
						turn_base(turn_id, input, observation.state, observation.observation_id),
						common,
					);
					return yield* UpsertItem(
						transaction,
						input.thread_id,
						{
							...item_base(
								`work:${turn_id}`,
								turn_id,
								input,
								observation.state,
								observation.observation_id,
							),
							...(observation.state === "completed" ||
							observation.state === "cancelled" ||
							observation.state === "failed"
								? { ended_at: input.occurred_at }
								: {}),
							started_at: input.occurred_at,
							status: lifecycle(observation.state),
							title: "Agent work",
							type: "work_session",
						},
						common,
					);
				});
			case "run_state":
			case "run_terminal":
				return yield* UpsertTurn(
					transaction,
					input.thread_id,
					turn_base(
						`run:${input.run_id}`,
						input,
						observation.state,
						observation.observation_id,
					),
					common,
				);
			case "plan":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`plan:${observation.observation_id}`,
							turn_id,
							input,
							"active",
							observation.observation_id,
						),
						type: "plan",
						state: "active",
						entries: observation.entries.map((entry) => ({
							id: entry.id,
							text: text(entry.text) || "Plan entry",
							state: entry.status === "in_progress" ? "active" : entry.status,
						})),
					},
					common,
				);
			case "approval":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`approval:${observation.approval_id}`,
							turn_id,
							input,
							observation.state === "requested" ? "waiting" : "completed",
							observation.observation_id,
						),
						type: "approval",
						interaction_id: observation.approval_id,
						prompt: text(observation.description) || "Approval requested",
						requested_at: input.occurred_at,
						state:
							observation.state === "requested"
								? "requested"
								: observation.approved
									? "approved"
									: "rejected",
						...(observation.state === "resolved"
							? {
									resolution: observation.approved ? "Approved" : "Rejected",
									resolved_at: input.occurred_at,
								}
							: {}),
					},
					common,
				);
			case "question":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`question:${observation.question_id}`,
							turn_id,
							input,
							observation.state === "requested" ? "waiting" : "completed",
							observation.observation_id,
						),
						type: "question",
						interaction_id: observation.question_id,
						prompt: text(observation.text) || "Question",
						requested_at: input.occurred_at,
						state: observation.state === "requested" ? "requested" : "answered",
						...(observation.answers
							? {
									resolution:
										text(observation.answers.flat().join("; ")) || "Answered",
									resolved_at: input.occurred_at,
								}
							: {}),
					},
					common,
				);
			case "compaction":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`compaction:${observation.observation_id}`,
							turn_id,
							input,
							observation.state === "completed" ? "completed" : "active",
							observation.observation_id,
						),
						type: "compaction",
						state: observation.state,
						portability: "provider_bound",
						...(observation.summary ? { summary: text(observation.summary) } : {}),
					},
					common,
				);
			case "retry":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`error:${observation.observation_id}`,
							turn_id,
							input,
							observation.will_retry ? "waiting" : "failed",
							observation.observation_id,
						),
						type: "error",
						message: text(observation.message) || "Provider error",
						retry: observation.will_retry
							? { kind: "scheduled", after_ms: 0, attempt: 0, max_attempts: 0 }
							: { kind: "none" },
					},
					common,
				);
			case "file":
				return yield* observation.action === "read"
					? UpsertItem(
							transaction,
							input.thread_id,
							{
								...item_base(
									`activity:${observation.observation_id}`,
									turn_id,
									input,
									"completed",
									observation.observation_id,
								),
								kind: "file",
								label: "Read file",
								status: "completed",
								detail: text(observation.path) || "Unknown file",
								type: "activity",
							},
							common,
						)
					: Effect.gen(function* () {
							const file_id = `file:${observation.observation_id}`;
							const change_set_id = `change-set:${observation.observation_id}`;
							yield* UpsertItem(
								transaction,
								input.thread_id,
								{
									...item_base(
										change_set_id,
										turn_id,
										input,
										"completed",
										observation.observation_id,
									),
									file_count: 1,
									file_ids: [file_id],
									state: "applied",
									summary: `Changed ${text(observation.path) || "file"}`,
									type: "change_set",
								},
								common,
							);
							return yield* UpsertItem(
								transaction,
								input.thread_id,
								{
									...item_base(
										file_id,
										turn_id,
										input,
										"completed",
										observation.observation_id,
									),
									change_set_id,
									diff: { kind: "unavailable" },
									operation: observation.action,
									path: text(observation.path) || "Unknown file",
									type: "file_change",
								},
								common,
							);
						});
			case "terminal_activity":
			case "tool":
			case "search":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`activity:${observation.observation_id}`,
							turn_id,
							input,
							observation._tag === "tool" && observation.action === "failed"
								? "failed"
								: "active",
							observation.observation_id,
						),
						type: "activity",
						kind: observation._tag,
						label:
							observation._tag === "tool"
								? text(observation.tool_name) || "Tool"
								: observation._tag === "search"
									? "Search"
									: "Terminal",
						status:
							observation._tag === "tool"
								? lifecycle(observation.action)
								: observation._tag === "search"
									? lifecycle(observation.state)
									: lifecycle(observation.state),
						...(observation._tag === "tool" && observation.detail
							? { detail: text(observation.detail) }
							: {}),
					},
					common,
				);
			case "native_action":
			case "protocol_diagnostic":
			case "process_diagnostic":
			case "usage":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`native:${observation.observation_id}`,
							turn_id,
							input,
							"completed",
							observation.observation_id,
						),
						type: "native_event",
						summary:
							observation._tag === "native_action"
								? text(observation.action) || "Native engine action"
								: observation._tag === "usage"
									? "Usage update"
									: text(observation.message) || "Engine diagnostic",
					},
					common,
				);
		}
	});

/** Applies journal facts which are canonical user-visible conversation input. */
export const ApplyJournalEvent = (transaction: any, event: EventEnvelope) =>
	Effect.gen(function* () {
		yield* EnsureThread(transaction, event.thread_id, event.sent_at);
		const admitted = yield* Admit(
			transaction,
			`event:${event.message_id}`,
			event.thread_id,
			event.sent_at,
			event.journal_sequence,
		);
		if (!admitted) return;
		const payload: any = event.payload;
		if (payload.type === "run.lifecycle" && event.run_id !== undefined) {
			const turn_id = `run:${event.run_id}`;
			const state = lifecycle(payload.state);
			const source = {
				observed_at: event.sent_at,
				journal_sequence: event.journal_sequence,
			};

			yield* UpsertTurn(
				transaction,
				event.thread_id,
				{
					...turn_base(
						turn_id,
						{
							thread_id: event.thread_id,
							run_id: event.run_id,
							occurred_at: event.sent_at,
						},
						state,
						event.message_id,
					),
					source_refs: source_refs(event.message_id, {
						event_id: event.message_id,
						journal_sequence: event.journal_sequence,
					}),
				},
				source,
			);
			yield* UpsertItem(
				transaction,
				event.thread_id,
				{
					...item_base(
						`work:${turn_id}`,
						turn_id,
						{
							thread_id: event.thread_id,
							run_id: event.run_id,
							occurred_at: event.sent_at,
						},
						state,
						event.message_id,
					),
					...(state === "completed" || state === "failed" || state === "cancelled"
						? { ended_at: event.sent_at }
						: {}),
					started_at: event.sent_at,
					status: state,
					title: "Agent work",
					type: "work_session",
					source_refs: source_refs(event.message_id, {
						event_id: event.message_id,
						journal_sequence: event.journal_sequence,
					}),
				},
				source,
			);
			return;
		}
		if (payload.type !== "thread.message_queued" && payload.type !== "thread.message_steering")
			return;
		const turn_id = `turn:user:${payload.message_id}`;
		const source = { observed_at: event.sent_at, journal_sequence: event.journal_sequence };
		yield* UpsertTurn(
			transaction,
			event.thread_id,
			{
				...turn_base(
					turn_id,
					{
						thread_id: event.thread_id,
						run_id: event.run_id ?? "journal",
						occurred_at: event.sent_at,
					},
					"completed",
					event.message_id,
				),
				source_refs: source_refs(payload.message_id, {
					event_id: event.message_id,
					journal_sequence: event.journal_sequence,
				}),
			},
			source,
		);
		yield* UpsertItem(
			transaction,
			event.thread_id,
			{
				...item_base(
					`message:${payload.message_id}`,
					turn_id,
					{
						thread_id: event.thread_id,
						run_id: event.run_id ?? "journal",
						occurred_at: event.sent_at,
					},
					"completed",
					event.message_id,
				),
				...(payload.attachments === undefined ? {} : { attachments: payload.attachments }),
				...(payload.content === undefined ? {} : { content: payload.content }),
				type: "user_message",
				text: text(payload.text) || "Message",
				source_refs: source_refs(payload.message_id, {
					event_id: event.message_id,
					journal_sequence: event.journal_sequence,
				}),
			},
			source,
		);
	});

/** Decodes a complete durable conversation snapshot. */
export const ReadConversationSnapshot = (transaction: any, thread_id: string) =>
	Effect.gen(function* () {
		const [thread] = yield* transaction
			.select()
			.from(ConversationThreads)
			.where(eq(ConversationThreads.thread_id, thread_id))
			.limit(1);
		if (!thread) return undefined;
		const turns = yield* transaction
			.select()
			.from(ConversationTurns)
			.where(eq(ConversationTurns.thread_id, thread_id))
			.orderBy(asc(ConversationTurns.ordinal));
		const items = yield* transaction
			.select()
			.from(ConversationItems)
			.where(eq(ConversationItems.thread_id, thread_id))
			.orderBy(asc(ConversationItems.ordinal));
		return yield* Decode(
			ConversationSnapshot,
			{
				conversation_id: `conversation:${thread_id}`,
				thread_id,
				schema_version: 1,
				journal_sequence: thread.journal_sequence,
				last_patch_sequence: thread.last_patch_sequence,
				updated_at: thread.updated_at,
				turns: yield* Effect.forEach(turns as ReadonlyArray<any>, (row) =>
					Decode(
						ConversationTurn,
						JSON.parse(row.entity_json),
						"stored conversation turn",
					),
				),
				items: yield* Effect.forEach(items as ReadonlyArray<any>, (row) =>
					Decode(
						ConversationItem,
						JSON.parse(row.entity_json),
						"stored conversation item",
					),
				),
			},
			"conversation snapshot",
		);
	});

export const ReadConversationPatches = (
	transaction: any,
	thread_id: string,
	after_sequence: number,
	maximum = conversation_patch_replay_batch_size,
) =>
	transaction
		.select()
		.from(ConversationPatches)
		.where(
			and(
				eq(ConversationPatches.thread_id, thread_id),
				gt(ConversationPatches.sequence, after_sequence),
			),
		)
		.orderBy(asc(ConversationPatches.sequence))
		.limit(Math.min(Math.max(1, maximum), conversation_patch_replay_batch_size))
		.pipe(
			Effect.flatMap((rows: ReadonlyArray<any>) =>
				Effect.forEach(rows, (row) =>
					Decode(
						ConversationPatch,
						JSON.parse(row.patch_json),
						"stored conversation patch",
					),
				),
			),
		);
