import { Effect, Option, Ref } from "effect";
import { describe, expect, it } from "vitest";

import {
	conversation_patch_replay_batch_size,
	conversation_patch_retention_limit,
	type ConversationReadModelFailure,
	ConversationReadModel,
} from "../../modules/backend/src/conversation";
import type {
	ConversationPatch,
	ConversationSnapshot,
	OutboundControlEnvelope,
} from "@artisan/protocol";
import type {
	ConnectionState,
	PendingSubscription,
	ReadyState,
} from "../../modules/backend/src/protocol/connection-state";
import { MakeConnectionConversationDelivery } from "../../modules/backend/src/protocol/subscriptions/conversation-delivery";
import { ConnectionSubscriptionControl } from "../../modules/backend/src/protocol/subscriptions/control";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const copies = 24;

const SubscriptionIds = (envelopes: ReadonlyArray<OutboundControlEnvelope>) =>
	envelopes.flatMap((envelope) =>
		"subscription_id" in envelope ? [envelope.subscription_id] : [],
	);

const Patch = (sequence: number) => ({ sequence }) as ConversationPatch;

const Snapshot = (thread_id: string, last_patch_sequence: number): ConversationSnapshot =>
	({
		conversation_id: `conversation:${thread_id}`,
		items: [],
		journal_sequence: 99,
		last_patch_sequence,
		schema_version: 1,
		thread_id,
		turns: [],
		updated_at: "2026-08-15T00:00:00.000Z",
	}) as ConversationSnapshot;

const State = (
	entries: ReadonlyArray<
		readonly [subscription_id: string, thread_id: string, patch_sequence: number]
	>,
): ReadyState => ({
	_tag: "Ready",
	acknowledged_cursors: {},
	acknowledged_journal_sequence: 0,
	connection_id: "connection_conversation_delivery",
	delivered_cursors: {},
	delivered_journal_sequence: 7,
	last_activity_ms: 0,
	stream_ticket: "ticket_conversation_delivery",
	/** Delivery only serves claimed subscriptions; registration installs both atomically. */
	subscription_claims: Object.fromEntries(
		entries.map(([subscription_id]) => [
			subscription_id,
			{ message_id: `subscribe_${subscription_id}` } as PendingSubscription,
		]),
	),
	subscriptions: Object.fromEntries(
		entries.map(([subscription_id, thread_id, patch_sequence]) => [
			subscription_id,
			{
				_tag: "conversation" as const,
				journal_sequence: 0,
				patch_sequence,
				sequence: 0,
				stream_id: `stream:${subscription_id}`,
				thread_id,
			},
		]),
	),
});

const Deliver = (
	current: ReadyState,
	read_patches: (
		thread_id: string,
		patch_sequence: number,
	) => Effect.Effect<ReadonlyArray<ConversationPatch>, ConversationReadModelFailure>,
	read_snapshot: (
		thread_id: string,
	) => Effect.Effect<
		{ readonly status: "available"; readonly snapshot: ConversationSnapshot },
		ConversationReadModelFailure
	>,
	/** Whether a claim still publishes; a lost claim swallows the publication. */
	publishes: (subscription_id: string) => boolean = () => true,
) =>
	Effect.gen(function* () {
		const envelopes = yield* Ref.make<ReadonlyArray<OutboundControlEnvelope>>([]);
		const state = yield* Ref.make<ConnectionState>(current);
		const delivery = yield* MakeConnectionConversationDelivery.pipe(
			Effect.provideService(
				ConversationReadModel,
				ConversationReadModel.of({
					ReadPatches: read_patches,
					ReadSnapshot: read_snapshot,
				} as never),
			),
			Effect.provideService(
				ConnectionSubscriptionControl,
				ConnectionSubscriptionControl.of({
					Enqueue: (envelope) =>
						Ref.update(envelopes, (current) => [...current, envelope]),
					EnqueueError: () => Effect.void,
					PublishClaim: (subscription_id, _claim, publication) =>
						publishes(subscription_id)
							? publication.pipe(Effect.map(Option.some))
							: Effect.succeed(Option.none()),
					state,
				}),
			),
			Effect.provideService(
				RuntimeMetadata,
				RuntimeMetadata.of({
					MakeId: () => Effect.succeed("message_conversation_delivery"),
					Now: Effect.succeed("2026-08-15T00:00:00.000Z"),
				} as never),
			),
		);
		const subscriptions = yield* delivery.EnqueuePatches(current);
		return { envelopes: yield* Ref.get(envelopes), subscriptions };
	});

describe("conversation delivery performance", () => {
	it("reads one short batch once for 24 equivalent cursors and still addresses every stream", async () => {
		const patch_calls = await Ref.make(0).pipe(Effect.runPromise);
		const current = State(
			Array.from({ length: copies }, (_, index) => [
				`subscription_${index}`,
				"thread_shared",
				0,
			]),
		);
		const result = await Effect.runPromise(
			Deliver(
				current,
				() => Ref.update(patch_calls, (count) => count + 1).pipe(Effect.as([Patch(1)])),
				(thread_id) =>
					Effect.succeed({ status: "available", snapshot: Snapshot(thread_id, 0) }),
			),
		);

		expect(await Effect.runPromise(Ref.get(patch_calls))).toBe(1);
		expect(result.envelopes).toHaveLength(copies);
		expect(new Set(SubscriptionIds(result.envelopes))).toHaveLength(copies);
		expect(result.envelopes.every((envelope) => envelope.kind === "conversation.patch")).toBe(
			true,
		);
	});

	it("reads each distinct batch cursor once while every equivalent stream advances", async () => {
		const calls = await Ref.make<ReadonlyArray<readonly [string, number]>>([]).pipe(
			Effect.runPromise,
		);
		const current = State(
			Array.from({ length: copies }, (_, index) => [
				`subscription_${index}`,
				"thread_shared",
				0,
			]),
		);
		const result = await Effect.runPromise(
			Deliver(
				current,
				(thread_id, patch_sequence) =>
					Ref.update(calls, (current) => [
						...current,
						[thread_id, patch_sequence] as const,
					]).pipe(
						Effect.as(
							patch_sequence === 0
								? Array.from(
										{ length: conversation_patch_replay_batch_size },
										(_, index) => Patch(index + 1),
									)
								: [Patch(patch_sequence + 1)],
						),
					),
				(thread_id) =>
					Effect.succeed({ status: "available", snapshot: Snapshot(thread_id, 0) }),
			),
		);

		expect(await Effect.runPromise(Ref.get(calls))).toEqual([
			["thread_shared", 0],
			["thread_shared", conversation_patch_replay_batch_size],
		]);
		expect(result.envelopes).toHaveLength(copies * 2);
		expect(
			Object.values(result.subscriptions).every(
				(subscription) =>
					subscription._tag === "conversation" &&
					subscription.patch_sequence === conversation_patch_replay_batch_size + 1,
			),
		).toBe(true);
	});

	it("shares a gap snapshot while preserving independently addressed snapshot streams", async () => {
		const snapshot_calls = await Ref.make(0).pipe(Effect.runPromise);
		const current = State(
			Array.from({ length: copies }, (_, index) => [
				`subscription_${index}`,
				"thread_gap",
				0,
			]),
		);
		const result = await Effect.runPromise(
			Deliver(
				current,
				() => Effect.succeed([Patch(2)]),
				(thread_id) =>
					Ref.update(snapshot_calls, (count) => count + 1).pipe(
						Effect.as({
							status: "available" as const,
							snapshot: Snapshot(thread_id, 42),
						}),
					),
			),
		);

		expect(await Effect.runPromise(Ref.get(snapshot_calls))).toBe(1);
		expect(result.envelopes).toHaveLength(copies);
		expect(new Set(SubscriptionIds(result.envelopes))).toHaveLength(copies);
		expect(
			result.envelopes.every((envelope) => envelope.kind === "conversation.snapshot"),
		).toBe(true);
		expect(
			Object.values(result.subscriptions).every(
				(subscription) =>
					subscription._tag === "conversation" && subscription.patch_sequence === 42,
			),
		).toBe(true);
	});

	it("drains a full retention window through patches without a snapshot fallback", async () => {
		const snapshot_calls = await Ref.make(0).pipe(Effect.runPromise);
		const current = State([["catching_up", "thread_behind", 0]]);
		const result = await Effect.runPromise(
			Deliver(
				current,
				(_thread_id, patch_sequence) =>
					Effect.succeed(
						patch_sequence >= conversation_patch_retention_limit
							? []
							: Array.from(
									{ length: conversation_patch_replay_batch_size },
									(_, index) => Patch(patch_sequence + index + 1),
								),
					),
				(thread_id) =>
					Ref.update(snapshot_calls, (count) => count + 1).pipe(
						Effect.as({
							status: "available" as const,
							snapshot: Snapshot(thread_id, conversation_patch_retention_limit),
						}),
					),
			),
		);

		expect(await Effect.runPromise(Ref.get(snapshot_calls))).toBe(0);
		expect(result.envelopes.every((envelope) => envelope.kind === "conversation.patch")).toBe(
			true,
		);
		expect(result.envelopes).toHaveLength(
			conversation_patch_retention_limit / conversation_patch_replay_batch_size,
		);
		expect(result.subscriptions.catching_up).toMatchObject({
			patch_sequence: conversation_patch_retention_limit,
		});
	});

	it("keeps cache lifetime to one delivery and isolates distinct thread and cursor reads", async () => {
		const calls = await Ref.make<ReadonlyArray<readonly [string, number]>>([]).pipe(
			Effect.runPromise,
		);
		const current = State([
			["shared_a", "thread_a", 0],
			["shared_b", "thread_a", 0],
			["cursor_b", "thread_a", 1],
			["thread_b", "thread_b", 0],
		]);
		const read_patches = (thread_id: string, patch_sequence: number) =>
			Ref.update(calls, (current) => [...current, [thread_id, patch_sequence] as const]).pipe(
				Effect.as([Patch(patch_sequence + 1)]),
			);
		const read_snapshot = (thread_id: string) =>
			Effect.succeed({ status: "available" as const, snapshot: Snapshot(thread_id, 0) });

		await Effect.runPromise(Deliver(current, read_patches, read_snapshot));
		await Effect.runPromise(Deliver(current, read_patches, read_snapshot));

		expect(await Effect.runPromise(Ref.get(calls))).toEqual([
			["thread_a", 0],
			["thread_a", 1],
			["thread_b", 0],
			["thread_a", 0],
			["thread_a", 1],
			["thread_b", 0],
		]);
	});

	/**
	 * There is no affected-threads filter to test any more, and that is the
	 * point: conversation patches are written outside the journal, so no event
	 * predicate can prove a wake carried none. Every wake consults every
	 * subscribed cursor, and a thread whose patches were written by a writer
	 * the old predicate missed is healed by whatever wakes the connection next.
	 */
	it("drains every subscribed conversation from its own cursor on every wake", async () => {
		const calls = await Ref.make<ReadonlyArray<string>>([]).pipe(Effect.runPromise);
		const current = State([
			["thread_a", "thread_a", 0],
			["thread_b", "thread_b", 0],
		]);
		const read_patches = (thread_id: string) =>
			Ref.update(calls, (current) => [...current, thread_id]).pipe(Effect.as([Patch(1)]));
		const read_snapshot = (thread_id: string) =>
			Effect.succeed({ status: "available" as const, snapshot: Snapshot(thread_id, 0) });

		const delivered = await Effect.runPromise(Deliver(current, read_patches, read_snapshot));

		expect(await Effect.runPromise(Ref.get(calls))).toEqual(["thread_a", "thread_b"]);
		expect(new Set(SubscriptionIds(delivered.envelopes))).toEqual(
			new Set(["thread_a", "thread_b"]),
		);
	});

	/**
	 * A claim that is no longer owned publishes nothing, and a cursor advanced
	 * past an envelope that never left the process makes the skipped patches
	 * unreachable forever — the client would wait on a window nobody will send.
	 */
	it("never advances a cursor past an envelope its claim refused to publish", async () => {
		const current = State([
			["thread_a", "thread_a", 0],
			["thread_b", "thread_b", 0],
		]);
		const read_patches = (_thread_id: string, patch_sequence: number) =>
			Effect.succeed(patch_sequence === 0 ? [Patch(1)] : []);
		const read_snapshot = (thread_id: string) =>
			Effect.succeed({ status: "available" as const, snapshot: Snapshot(thread_id, 0) });

		const delivered = await Effect.runPromise(
			Deliver(
				current,
				read_patches,
				read_snapshot,
				(subscription_id) => subscription_id !== "thread_b",
			),
		);

		expect(SubscriptionIds(delivered.envelopes)).toEqual(["thread_a"]);
		expect(delivered.subscriptions.thread_a).toMatchObject({
			patch_sequence: 1,
			sequence: 1,
		});
		expect(delivered.subscriptions.thread_b).toMatchObject({
			patch_sequence: 0,
			sequence: 0,
		});
	});
});
