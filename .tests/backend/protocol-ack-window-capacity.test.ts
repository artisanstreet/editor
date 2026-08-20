import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Chunk, Deferred, Effect, Fiber, Layer, Ref, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	AckEnvelope,
	EventEnvelope,
	HelloEnvelope,
	OutboundControlEnvelope,
	ReplayEnvelope,
	SubscribeEnvelope,
	ThreadListQueryEnvelope,
} from "@artisan/protocol";
import { make_backend_runtime, ProtocolServer, type ProtocolConnection } from "@artisan/backend";

import { AgentGraphOrchestrator } from "../../modules/backend/src/orchestration/agent-graph-orchestrator";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { TranscriptReadModel } from "../../modules/backend/src/persistence/transcript-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { SurfaceService } from "../../modules/backend/src/surfaces/service";
import { WorkspaceChangeRepository } from "../../modules/backend/src/workspace/changes/repository";
import type {
	ConnectionState,
	ProjectionSubscription,
	ReadyState,
} from "../../modules/backend/src/protocol/connection-state";
import { ConnectionConversationDelivery } from "../../modules/backend/src/protocol/subscriptions/conversation-delivery";
import {
	ConnectionSubscriptionControl,
	MakeSubscriptionControlHandlers,
} from "../../modules/backend/src/protocol/subscriptions/control";
import { MakeLiveEventDelivery } from "../../modules/backend/src/protocol/subscriptions/live-events";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const Event = (journal_sequence: number): EventEnvelope =>
	({
		journal_sequence,
		kind: "event",
		message_id: `event_${journal_sequence}`,
		origin: "backend",
		payload: { title: `Thread ${journal_sequence}`, type: "thread.created" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-08-15T00:00:00.000Z",
		sequence: journal_sequence,
		stream_id: "thread:ack-window",
		thread_id: "thread_ack_window",
	}) as EventEnvelope;

/** The retransmission window is a Chunk; assertions read it as a plain list. */
const UnacknowledgedArray = (state: ConnectionState) =>
	state._tag === "Ready"
		? Chunk.toArray(state.unacknowledged_events ?? Chunk.empty())
		: undefined;

const Ready = (): ReadyState => ({
	_tag: "Ready",
	acknowledged_cursors: {},
	acknowledged_journal_sequence: 0,
	connection_id: "connection_ack_window",
	delivered_cursors: {},
	delivered_journal_sequence: 0,
	last_activity_ms: 0,
	stream_ticket: "ticket_ack_window",
	subscriptions: {},
	unacknowledged_events: Chunk.empty(),
});

const Ack = (journal_sequence: number): AckEnvelope =>
	({
		kind: "ack",
		message_id: `ack_${journal_sequence}`,
		origin: "frontend",
		payload: {
			event_cursors: [{ sequence: journal_sequence, stream_id: "thread:ack-window" }],
			journal_sequence,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-08-15T00:00:00.000Z",
	}) as AckEnvelope;

const Replay = (): ReplayEnvelope =>
	({
		kind: "replay",
		message_id: "replay_ack_window",
		origin: "frontend",
		payload: { after_journal_sequence: 0 },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-08-15T00:00:00.000Z",
	}) as ReplayEnvelope;

const MakeMetadata = Layer.succeed(
	RuntimeMetadata,
	RuntimeMetadata.of({
		MakeId: (prefix: string) => Effect.succeed(`${prefix}_ack_window`),
		Now: Effect.succeed("2026-08-15T00:00:00.000Z"),
	} as never),
);

const project_subscription = {
	_tag: "project.list",
	sequence: 7,
	stream_id: "project:ack-window",
} satisfies ProjectionSubscription;

const MakeControl = (
	state: Ref.Ref<ConnectionState>,
	emitted: Array<OutboundControlEnvelope>,
	errors: Array<{ readonly code: string; readonly retryable: boolean }>,
	enqueue?: (envelope: OutboundControlEnvelope) => Effect.Effect<void>,
) =>
	ConnectionSubscriptionControl.of({
		state,
		Enqueue: enqueue ?? ((envelope) => Effect.sync(() => void emitted.push(envelope))),
		EnqueueError: (_current, code, _message, retryable) =>
			Effect.sync(() => void errors.push({ code, retryable })),
	});

const MakeLiveDelivery = (
	state: Ref.Ref<ConnectionState>,
	emitted: Array<OutboundControlEnvelope>,
	errors: Array<{ readonly code: string; readonly retryable: boolean }>,
	enqueue?: (envelope: OutboundControlEnvelope) => Effect.Effect<void>,
) =>
	MakeLiveEventDelivery.pipe(
		Effect.provideService(
			ConnectionSubscriptionControl,
			MakeControl(state, emitted, errors, enqueue),
		),
		Effect.provideService(
			ConnectionConversationDelivery,
			ConnectionConversationDelivery.of({
				EnqueuePatches: (current) => Effect.succeed(current.subscriptions),
			}),
		),
		Effect.provideService(AgentGraphOrchestrator, AgentGraphOrchestrator.of({} as never)),
		Effect.provideService(OrchestrationRepository, OrchestrationRepository.of({} as never)),
		Effect.provideService(SurfaceService, SurfaceService.of({} as never)),
		Effect.provideService(ThreadReadModel, ThreadReadModel.of({} as never)),
		Effect.provideService(TranscriptReadModel, TranscriptReadModel.of({} as never)),
		Effect.provideService(WorkspaceChangeRepository, WorkspaceChangeRepository.of({} as never)),
		Effect.provide(MakeMetadata),
	);

const MakeHello = (): HelloEnvelope => ({
	kind: "hello",
	message_id: "hello_ack_window_resume",
	origin: "frontend",
	payload: {
		event_cursors: [],
		last_journal_sequence: 0,
		resume_mode: "resume",
		supported_protocol_versions: [1],
	},
	schema_version: 1,
	sent_at: "2026-08-15T00:00:00.000Z",
});

const MakeSubscribe = (): SubscribeEnvelope => ({
	kind: "subscribe",
	message_id: "subscribe_ack_window",
	origin: "frontend",
	payload: { type: "thread.list" },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T00:00:00.000Z",
	subscription_id: "subscription_ack_window",
});

const MakeQuery = (): ThreadListQueryEnvelope => ({
	kind: "thread.list.query",
	message_id: "query_ack_window",
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T00:00:00.000Z",
});

const Take = (connection: ProtocolConnection, count: number) =>
	connection.Outbound.pipe(Stream.take(count), Stream.runCollect);

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("protocol acknowledgement delivery", () => {
	it("delivers an arbitrarily sized live batch and retains ACK correctness", async () => {
		const state = await Effect.runPromise(Ref.make<ConnectionState>(Ready()));
		const emitted: Array<OutboundControlEnvelope> = [];
		const errors: Array<{ readonly code: string; readonly retryable: boolean }> = [];
		const delivery = await Effect.runPromise(MakeLiveDelivery(state, emitted, errors));
		const control = await Effect.runPromise(
			MakeSubscriptionControlHandlers.pipe(
				Effect.provideService(
					ConnectionSubscriptionControl,
					MakeControl(state, emitted, errors),
				),
				Effect.provideService(JournalStore, JournalStore.of({} as never)),
				Effect.provide(MakeMetadata),
			),
		);

		await Effect.runPromise(delivery.DeliverLiveEvents([Event(1), Event(2), Event(3)]));
		await Effect.runPromise(control.HandleAck(Ack(2), Ready()));

		expect(emitted.filter((envelope) => envelope.kind === "event")).toHaveLength(3);
		expect(errors).toEqual([]);
		const acked = await Effect.runPromise(Ref.get(state));
		expect(acked).toMatchObject({
			acknowledged_journal_sequence: 2,
			delivered_journal_sequence: 3,
		});
		expect(UnacknowledgedArray(acked)).toEqual([Event(3)]);
	});

	it("delivers a complete manual replay batch", async () => {
		const state = await Effect.runPromise(Ref.make<ConnectionState>(Ready()));
		const emitted: Array<OutboundControlEnvelope> = [];
		const errors: Array<{ readonly code: string; readonly retryable: boolean }> = [];
		const handlers = await Effect.runPromise(
			MakeSubscriptionControlHandlers.pipe(
				Effect.provideService(
					ConnectionSubscriptionControl,
					MakeControl(state, emitted, errors),
				),
				Effect.provideService(
					JournalStore,
					JournalStore.of({
						ReadReplay: () => Effect.succeed([Event(1), Event(2), Event(3)]),
					} as never),
				),
				Effect.provide(MakeMetadata),
			),
		);

		await Effect.runPromise(handlers.HandleReplay(Replay(), Ready()));

		expect(emitted.slice(0, 3)).toEqual([Event(1), Event(2), Event(3)]);
		expect(emitted.at(-1)).toMatchObject({ kind: "replay.complete" });
		expect(errors).toEqual([]);
		const replayed = await Effect.runPromise(Ref.get(state));
		expect(replayed).toMatchObject({ delivered_journal_sequence: 3 });
		expect(UnacknowledgedArray(replayed)).toEqual([Event(1), Event(2), Event(3)]);
	});

	it("merges live delivery into the latest connection state after an enqueue yield", async () => {
		const state = await Effect.runPromise(
			Ref.make<ConnectionState>({
				...Ready(),
				pending_heartbeat: {
					deadline_ms: 10,
					message_id: "heartbeat_ack_window",
					nonce: "nonce_ack_window",
				},
			}),
		);
		const emitted: Array<OutboundControlEnvelope> = [];
		const errors: Array<{ readonly code: string; readonly retryable: boolean }> = [];
		const enqueue_started = await Effect.runPromise(Deferred.make<void>());
		const enqueue_release = await Effect.runPromise(Deferred.make<void>());
		const delivery = await Effect.runPromise(
			MakeLiveDelivery(state, emitted, errors, (envelope) =>
				Deferred.succeed(enqueue_started, undefined).pipe(
					Effect.andThen(Deferred.await(enqueue_release)),
					Effect.tap(() => Effect.sync(() => void emitted.push(envelope))),
				),
			),
		);

		const final = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const delivery_fiber = yield* Effect.forkScoped(
						delivery.DeliverLiveEvents([Event(1)]),
					);
					yield* Deferred.await(enqueue_started);
					yield* Ref.update(state, (current) => {
						if (current._tag !== "Ready") return current;
						const { pending_heartbeat: _, ...without_heartbeat } = current;
						return {
							...without_heartbeat,
							last_activity_ms: 99,
							subscriptions: {
								...without_heartbeat.subscriptions,
								project_ack_window: project_subscription,
							},
						};
					});
					yield* Deferred.succeed(enqueue_release, undefined);
					yield* Fiber.join(delivery_fiber);
					return yield* Ref.get(state);
				}),
			),
		);

		expect(final).toMatchObject({
			delivered_journal_sequence: 1,
			last_activity_ms: 99,
			subscriptions: {
				project_ack_window: { sequence: 7, stream_id: "project:ack-window" },
			},
		});
		expect(UnacknowledgedArray(final)).toEqual([Event(1)]);
		expect(final).not.toHaveProperty("pending_heartbeat");
		expect(emitted).toEqual([Event(1)]);
		expect(errors).toEqual([]);
	});

	it("replays an oversized resume tail without falling back to a baseline", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-ack-window-"));
		temporary_directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
		});

		try {
			await runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					for (const sequence of [1, 2, 3]) {
						yield* journal.AcceptThreadCreate({
							kind: "command",
							message_id: `seed_ack_window_${sequence}`,
							origin: "frontend",
							payload: { title: `Seed ${sequence}`, type: "thread.create" },
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-08-15T00:00:00.000Z",
							thread_id: `thread_ack_window_${sequence}`,
						});
					}
				}),
			);
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* connection.Receive(MakeHello());
						const handshake = yield* Take(connection, 6);
						const welcome = handshake[0];
						if (welcome?.kind !== "welcome")
							return yield* Effect.die("missing welcome");
						yield* connection.Receive(MakeSubscribe());
						const subscription = yield* Take(connection, 2);
						yield* connection.Receive({
							...Ack(welcome.payload.journal_sequence),
							message_id: "ack_fresh_resume_baseline",
							payload: {
								event_cursors: welcome.payload.current_event_cursors,
								journal_sequence: welcome.payload.journal_sequence,
							},
						});
						yield* connection.Receive(MakeQuery());
						const query = yield* Take(connection, 1);

						return { handshake, query, subscription };
					}),
				),
			);

			expect(output.handshake[0]?.kind).toBe("welcome");
			expect(
				output.handshake.filter((envelope) => envelope.kind === "event").length,
			).toBeGreaterThan(2);
			expect(output.handshake.some((envelope) => envelope.kind === "replay.complete")).toBe(
				true,
			);
			/** Three seeds precede the server layer, so the guidance anchor takes sequence 4. */
			expect(output.handshake[0]).toMatchObject({
				payload: {
					current_event_cursors: expect.arrayContaining([
						{ sequence: 1, stream_id: "thread:thread_ack_window_1" },
					]),
					journal_sequence: 4,
				},
			});
			expect(output.subscription.map((envelope) => envelope.kind)).toEqual([
				"subscription.started",
				"thread.list.snapshot",
			]);
			expect(output.query).toMatchObject([
				{ correlation_id: "query_ack_window", kind: "thread.list.query.result" },
			]);
		} finally {
			await runtime.dispose();
		}
	});
});
