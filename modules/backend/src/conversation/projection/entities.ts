import { Effect, Schema } from "effect";
import { eq, sql } from "drizzle-orm";

import { ConversationItem, ConversationPatch, ConversationTurn } from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import {
	ConversationItems,
	ConversationPatches,
	ConversationSources,
	ConversationThreads,
	ConversationTurns,
} from "../../persistence/tables";
import { ConversationProjectionError } from "./domain";

const JsonValue = Schema.UnknownFromJsonString;

export const Decode = <S extends Schema.Constraint>(
	schema: S,
	value: unknown,
	context: string,
): Effect.Effect<S["Type"], ConversationProjectionError, S["DecodingServices"]> =>
	Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value).pipe(
		Effect.mapError(() => new ConversationProjectionError(`Invalid ${context}`)),
	);

export const DecodeJson = <S extends Schema.Constraint>(
	schema: S,
	value: string,
	context: string,
): Effect.Effect<S["Type"], ConversationProjectionError, S["DecodingServices"]> =>
	Schema.decodeUnknownEffect(JsonValue)(value).pipe(
		Effect.flatMap(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })),
		Effect.mapError(() => new ConversationProjectionError(`Invalid ${context}`)),
	);

export interface ProjectionEntityInput {
	readonly id: string;
	readonly type: string;
	readonly [key: string]: unknown;
}

export const EnsureThread = (transaction: DatabaseClient, thread_id: string, updated_at: string) =>
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

const AllocateOrdinal = (transaction: DatabaseClient, thread_id: string) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.update(ConversationThreads)
			.set({ next_ordinal: sql`${ConversationThreads.next_ordinal} + 1` })
			.where(eq(ConversationThreads.thread_id, thread_id))
			.returning({ next_ordinal: ConversationThreads.next_ordinal });
		const allocated = rows.at(0);
		if (allocated === undefined)
			return yield* Effect.fail(
				new ConversationProjectionError("Conversation thread is missing"),
			);
		return allocated.next_ordinal - 1;
	});

export const Emit = (
	transaction: DatabaseClient,
	thread_id: string,
	updated_at: string,
	patch: Readonly<Record<string, unknown>>,
	journal_sequence?: number,
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
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
		const allocated = rows.at(0);
		if (allocated === undefined)
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

export const Admit = (
	transaction: DatabaseClient,
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
		.pipe(Effect.map((rows) => rows.length > 0));

/**
 * Guarantees the turn row exists without the read-decode-compare cost of a
 * full upsert. Streamed deltas never change a turn's lifecycle, run, or
 * agent, so any lifecycle refresh is deferred to the next non-delta
 * observation on the same turn.
 */
export const EnsureTurn = (
	transaction: DatabaseClient,
	thread_id: string,
	turn: ProjectionEntityInput,
	source: { observed_at: string; journal_sequence?: number },
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.select({ turn_id: ConversationTurns.turn_id })
			.from(ConversationTurns)
			.where(eq(ConversationTurns.turn_id, turn.id))
			.limit(1);
		if (rows.length > 0) return;
		yield* UpsertTurn(transaction, thread_id, turn, source);
	});

export const UpsertTurn = (
	transaction: DatabaseClient,
	thread_id: string,
	turn: ProjectionEntityInput,
	source: { observed_at: string; journal_sequence?: number },
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.select()
			.from(ConversationTurns)
			.where(eq(ConversationTurns.turn_id, turn.id))
			.limit(1);
		const existing = rows.at(0);
		if (existing === undefined) {
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
		const prior = yield* DecodeJson(
			ConversationTurn,
			existing.entity_json,
			"stored conversation turn",
		);
		if (["completed", "failed", "cancelled"].includes(prior.lifecycle)) return prior;
		const candidate = yield* Decode(
			ConversationTurn,
			{ ...turn, ordinal: prior.ordinal, revision: prior.revision },
			"conversation turn candidate",
		);
		if (
			prior.lifecycle === candidate.lifecycle &&
			prior.run_id === candidate.run_id &&
			prior.agent_id === candidate.agent_id
		)
			return prior;
		const entity = yield* Decode(
			ConversationTurn,
			{ ...prior, ...candidate, ordinal: prior.ordinal, revision: prior.revision + 1 },
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

export const UpsertItem = (
	transaction: DatabaseClient,
	thread_id: string,
	item: ProjectionEntityInput,
	source: { observed_at: string; journal_sequence?: number },
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.select()
			.from(ConversationItems)
			.where(eq(ConversationItems.item_id, item.id))
			.limit(1);
		const existing = rows.at(0);
		if (existing === undefined) {
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
		const prior = yield* DecodeJson(
			ConversationItem,
			existing.entity_json,
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
