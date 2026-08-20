import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Fiber, Layer, Option, Pull, Ref, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	HelloEnvelope,
	OutboundControlEnvelope,
	SubscribeEnvelope,
	UnsubscribeEnvelope,
} from "@artisan/protocol";
import {
	make_backend_runtime,
	OrchestrationRecoveryGate,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";

import type {
	ConnectionState,
	ReadyState,
} from "../../modules/backend/src/protocol/connection-state";
import {
	ConnectionSubscriptionControl,
	MakeSubscriptionControlHandlers,
} from "../../modules/backend/src/protocol/subscriptions/control";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { ProjectCatalog } from "../../modules/backend/src/projects/project-catalog";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const MakeDatabasePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-subscription-generation-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
};

const MakeHello = (): HelloEnvelope => ({
	kind: "hello",
	message_id: "hello",
	origin: "frontend",
	payload: {
		event_cursors: [],
		last_journal_sequence: 0,
		resume_mode: "fresh",
		supported_protocol_versions: [1],
	},
	schema_version: 1,
	sent_at: "2026-08-15T08:00:00.000Z",
});

const MakeSubscribe = (
	message_id: string,
	subscription_id: string,
	type: "project.list" | "thread.list" = "thread.list",
): SubscribeEnvelope => ({
	kind: "subscribe",
	message_id,
	origin: "frontend",
	payload: { type },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T08:00:00.000Z",
	subscription_id,
});

const MakeUnsubscribe = (message_id: string, subscription_id: string): UnsubscribeEnvelope => ({
	kind: "unsubscribe",
	message_id,
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T08:00:00.000Z",
	subscription_id,
});

interface OutboundReader {
	readonly buffered: Array<OutboundControlEnvelope>;
	readonly pull: Pull.Pull<ReadonlyArray<OutboundControlEnvelope>, never>;
}

const outbound_readers = new WeakMap<ProtocolConnection, OutboundReader>();

const ReadOne = (reader: OutboundReader): Effect.Effect<OutboundControlEnvelope> =>
	Effect.suspend(() => {
		const buffered = reader.buffered.shift();
		if (buffered !== undefined) return Effect.succeed(buffered);
		return reader.pull.pipe(
			Effect.flatMap((batch) => {
				reader.buffered.push(...batch);
				return ReadOne(reader);
			}),
			Effect.catch(() => Effect.die("connection output ended unexpectedly")),
		);
	});

/** A single stream puller preserves every envelope when the queue yields a batch. */
const ReceiveOne = (connection: ProtocolConnection) =>
	Effect.gen(function* () {
		const existing = outbound_readers.get(connection);
		if (existing !== undefined) return yield* ReadOne(existing);
		const reader: OutboundReader = {
			buffered: [],
			pull: yield* Stream.toPull(connection.Outbound),
		};
		outbound_readers.set(connection, reader);
		return yield* ReadOne(reader);
	});

const ReceiveUntil = (
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) =>
	Effect.gen(function* () {
		while (true) {
			const envelope = yield* ReceiveOne(connection);
			if (predicate(envelope)) return envelope;
		}
	});

const ReceiveMatchingWithin = (
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) =>
	Effect.gen(function* () {
		while (true) {
			const envelope = yield* ReceiveOne(connection);
			if (predicate(envelope)) return envelope;
		}
	}).pipe(Effect.timeoutOption("30 millis"));

const Bootstrap = (connection: ProtocolConnection) =>
	Effect.gen(function* () {
		const negotiation = yield* connection.Receive(MakeHello()).pipe(Effect.forkChild);
		yield* ReceiveUntil(connection, (envelope) => envelope.kind === "welcome");
		yield* ReceiveUntil(connection, (envelope) => envelope.kind === "replay.complete");
		yield* Fiber.join(negotiation);
	});

const SetRecoveryAwait = (
	runtime: ReturnType<typeof make_backend_runtime>,
	Await: Effect.Effect<void, unknown>,
) =>
	runtime.runPromise(
		Effect.gen(function* () {
			const gate = yield* OrchestrationRecoveryGate;
			yield* Effect.sync(() => {
				Object.defineProperty(gate, "Await", {
					configurable: true,
					value: Await,
					writable: true,
				});
			});
			return gate;
		}),
	);

const MakeSignalledMetadata = (remaining: Ref.Ref<number>, reached: Deferred.Deferred<void>) => {
	const identifiers = Ref.makeUnsafe(0);
	return Layer.succeed(
		RuntimeMetadata,
		RuntimeMetadata.of({
			instance_id: "subscription-test",
			MakeId: (prefix) =>
				Ref.getAndUpdate(identifiers, (count) => count + 1).pipe(
					Effect.flatMap((count) =>
						prefix !== "message"
							? Effect.succeed(`${prefix}_${count}`)
							: Ref.modify(
									remaining,
									(value) => [value === 1, Math.max(0, value - 1)] as const,
								).pipe(
									Effect.tap((should_signal) =>
										should_signal
											? Deferred.succeed(reached, undefined)
											: Effect.void,
									),
									Effect.as(`message_${count}`),
								),
					),
				),
			Now: Effect.succeed("2026-08-15T08:00:00.000Z"),
		}),
	);
};

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const MakeReady = (): ReadyState => ({
	_tag: "Ready",
	acknowledged_cursors: {},
	acknowledged_journal_sequence: 0,
	connection_id: "connection",
	delivered_cursors: {},
	delivered_journal_sequence: 0,
	last_activity_ms: 0,
	pending_subscriptions: {},
	stream_ticket: "ticket",
	subscription_claims: {},
	subscriptions: {},
});

const MakeHandlers = (state: Ref.Ref<ConnectionState>) =>
	MakeSubscriptionControlHandlers.pipe(
		Effect.provideService(
			ConnectionSubscriptionControl,
			ConnectionSubscriptionControl.of({
				Enqueue: () => Effect.void,
				EnqueueError: () => Effect.void,
				state,
			}),
		),
		Effect.provideService(JournalStore, JournalStore.of({} as never)),
		Effect.provideService(
			RuntimeMetadata,
			RuntimeMetadata.of({
				instance_id: "test",
				MakeId: () => Effect.succeed("message"),
				Now: Effect.succeed("2026-08-15T08:00:00.000Z"),
			}),
		),
	);

describe("recovery-gated projection subscription claims", () => {
	it("accepts independent active ids, preserves duplicate precedence, and releases only after stopped", async () => {
		const buffered_match = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const stopped: OutboundControlEnvelope = {
						correlation_id: "stop",
						kind: "subscription.stopped",
						message_id: "stopped",
						origin: "backend",
						payload: {},
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-08-15T08:00:00.000Z",
						subscription_id: "buffered",
					};
					const late: OutboundControlEnvelope = {
						correlation_id: "late",
						kind: "subscription.started",
						message_id: "late",
						origin: "backend",
						payload: { stream_id: "buffered" },
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-08-15T08:00:00.000Z",
						subscription_id: "buffered",
					};
					const connection = {
						Outbound: Stream.fromIterable([stopped]),
					} as ProtocolConnection;
					yield* ReceiveUntil(
						connection,
						(envelope) => envelope.kind === "subscription.stopped",
					);
					/** Simulates a prohibited envelope buffered behind the accepted stop. */
					const reader = outbound_readers.get(connection);
					if (reader === undefined)
						return yield* Effect.die("outbound reader was not installed");
					reader.buffered.push(late);
					return yield* ReceiveMatchingWithin(
						connection,
						(envelope) =>
							envelope.kind === "subscription.started" &&
							envelope.subscription_id === "buffered",
					);
				}),
			),
		);
		expect(Option.getOrUndefined(buffered_match)).toMatchObject({
			correlation_id: "late",
		});

		const database_path = await MakeDatabasePath();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: {
				heartbeat_interval_ms: 60_000,
				heartbeat_timeout_ms: 60_000,
			},
		});

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* Bootstrap(connection);

						yield* connection.Receive(MakeSubscribe("first", "first"));
						yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "thread.list.snapshot" &&
								envelope.subscription_id === "first",
						);
						yield* connection.Receive(MakeSubscribe("duplicate", "first"));
						const duplicate = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "protocol.error" &&
								envelope.correlation_id === "duplicate",
						);
						yield* connection.Receive(MakeSubscribe("second", "second"));
						const second_started = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.started" &&
								envelope.correlation_id === "second",
						);
						yield* connection.Receive(MakeUnsubscribe("stop", "first"));
						const stopped = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.stopped" &&
								envelope.correlation_id === "stop",
						);
						return { duplicate, second_started, stopped };
					}),
				),
			);

			expect(output.duplicate).toMatchObject({
				payload: { code: "subscription.already_exists", retryable: false },
			});
			expect(output.stopped).toMatchObject({ subscription_id: "first" });
			expect(output.second_started).toMatchObject({ subscription_id: "second" });
		} finally {
			await runtime.dispose();
		}
	});

	it("holds pipelined reuse behind a full old terminal publication without blocking receive", async () => {
		const database_path = await MakeDatabasePath();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: {
				heartbeat_interval_ms: 60_000,
				heartbeat_timeout_ms: 60_000,
			},
		});

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* Bootstrap(connection);
						yield* connection.Receive(MakeSubscribe("old-start", "shared"));
						const old_started = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.started" &&
								envelope.correlation_id === "old-start",
						);
						/** The old terminal publication still fences a pipelined replacement. */
						yield* connection.Receive(MakeUnsubscribe("old-stop", "shared"));
						yield* connection.Receive(MakeSubscribe("replacement", "shared"));

						const stopped = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.stopped" &&
								envelope.correlation_id === "old-stop",
						);
						const replacement_started = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.started" &&
								envelope.correlation_id === "replacement",
						);
						const replacement_snapshot = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "thread.list.snapshot" &&
								envelope.subscription_id === "shared",
						);
						return { old_started, replacement_snapshot, replacement_started, stopped };
					}),
				),
			);

			expect(output.old_started).toMatchObject({
				kind: "subscription.started",
				correlation_id: "old-start",
			});
			expect(output.stopped).toMatchObject({ correlation_id: "old-stop" });
			expect(output.replacement_started).toMatchObject({ correlation_id: "replacement" });
			expect(output.replacement_snapshot).toMatchObject({ subscription_id: "shared" });
		} finally {
			await runtime.dispose();
		}
	});

	it("claims before forked recovery and lets immediate unsubscribe prevent every late frame", async () => {
		const database_path = await MakeDatabasePath();
		const release_recovery = await Effect.runPromise(Deferred.make<void>());
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: {
				heartbeat_interval_ms: 60_000,
				heartbeat_timeout_ms: 60_000,
			},
		});
		await SetRecoveryAwait(runtime, Deferred.await(release_recovery));

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* Bootstrap(connection);

						yield* connection.Receive(MakeSubscribe("subscribe-old", "shared"));
						yield* connection.Receive(MakeUnsubscribe("unsubscribe-old", "shared"));
						const stopped = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.stopped" &&
								envelope.correlation_id === "unsubscribe-old",
						);
						yield* Deferred.succeed(release_recovery, undefined);
						const late = yield* ReceiveMatchingWithin(
							connection,
							(envelope) =>
								"subscription_id" in envelope &&
								envelope.subscription_id === "shared",
						);

						/** Reuse immediately while the cancelled handler may still be finalizing. */
						yield* connection.Receive(MakeSubscribe("subscribe-new", "shared"));
						const replacement_started = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.started" &&
								envelope.correlation_id === "subscribe-new",
						);
						const replacement_snapshot = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "thread.list.snapshot" &&
								envelope.subscription_id === "shared",
						);

						return { late, replacement_snapshot, replacement_started, stopped };
					}),
				),
			);

			expect(output.stopped).toMatchObject({
				correlation_id: "unsubscribe-old",
				kind: "subscription.stopped",
				subscription_id: "shared",
			});
			expect(Option.getOrUndefined(output.late)).toBeUndefined();
			expect(output.replacement_started).toMatchObject({ correlation_id: "subscribe-new" });
			expect(output.replacement_snapshot).toMatchObject({ subscription_id: "shared" });
		} finally {
			await runtime.dispose();
		}
	});

	it("publishes one recovery error and clears the claim before the error is observable", async () => {
		const database_path = await MakeDatabasePath();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: {
				heartbeat_interval_ms: 60_000,
				heartbeat_timeout_ms: 60_000,
			},
		});
		const recovery_gate = await SetRecoveryAwait(runtime, Effect.void);

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* Bootstrap(connection);
						yield* Effect.sync(() => {
							Object.defineProperty(recovery_gate, "Await", {
								configurable: true,
								value: Effect.fail(new Error("held recovery failed")),
								writable: true,
							});
						});
						yield* connection.Receive(MakeSubscribe("subscribe-failed", "retryable"));
						const failure = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "protocol.error" &&
								envelope.correlation_id === "subscribe-failed",
						).pipe(Effect.timeout("2 seconds"));

						yield* connection.Receive(
							MakeUnsubscribe("unsubscribe-after-failure", "retryable"),
						);
						const residual = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "protocol.error" &&
								envelope.correlation_id === "unsubscribe-after-failure",
						).pipe(Effect.timeout("2 seconds"));
						const duplicate_failure = yield* ReceiveMatchingWithin(
							connection,
							(envelope) =>
								envelope.kind === "protocol.error" &&
								envelope.correlation_id === "subscribe-failed",
						);
						return { duplicate_failure, failure, residual };
					}),
				),
			);

			expect(output.failure).toMatchObject({
				correlation_id: "subscribe-failed",
				kind: "protocol.error",
				payload: { code: "orchestration.recovery_unavailable", retryable: true },
			});
			expect(output.residual).toMatchObject({
				correlation_id: "unsubscribe-after-failure",
				kind: "protocol.error",
				payload: { code: "subscription.not_found" },
			});
			expect(Option.getOrUndefined(output.duplicate_failure)).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("interrupts a held recovery waiter on close without late publication", async () => {
		const database_path = await MakeDatabasePath();
		const release_recovery = await Effect.runPromise(Deferred.make<void>());
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: {
				heartbeat_interval_ms: 60_000,
				heartbeat_timeout_ms: 60_000,
			},
		});
		await SetRecoveryAwait(runtime, Deferred.await(release_recovery));

		try {
			const after_close = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* Bootstrap(connection);
						const output = yield* connection.Outbound.pipe(
							Stream.runCollect,
							Effect.forkChild,
						);
						yield* connection.Receive(MakeSubscribe("subscribe-close", "closing"));
						yield* connection.Close;
						yield* Deferred.succeed(release_recovery, undefined);
						return yield* Fiber.join(output);
					}),
				),
			);

			expect(after_close).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("orders an in-flight snapshot publication entirely before stopped", async () => {
		const database_path = await MakeDatabasePath();
		const release_recovery = await Effect.runPromise(Deferred.make<void>());
		const publication_reached = await Effect.runPromise(Deferred.make<void>());
		const signal_remaining = await Effect.runPromise(Ref.make(0));
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: {
				heartbeat_interval_ms: 60_000,
				heartbeat_timeout_ms: 60_000,
			},
			runtime_metadata: MakeSignalledMetadata(signal_remaining, publication_reached),
		});
		await SetRecoveryAwait(runtime, Deferred.await(release_recovery));

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* Bootstrap(connection);
						yield* Ref.set(signal_remaining, 2);
						yield* connection.Receive(MakeSubscribe("subscribe-blocked", "blocked"));
						yield* Deferred.succeed(release_recovery, undefined);
						yield* Deferred.await(publication_reached);
						yield* connection.Receive(
							MakeUnsubscribe("unsubscribe-blocked", "blocked"),
						);
						const envelopes = [
							yield* ReceiveOne(connection),
							yield* ReceiveOne(connection),
							yield* ReceiveOne(connection),
						];
						const late = yield* ReceiveMatchingWithin(
							connection,
							(envelope) =>
								"subscription_id" in envelope &&
								envelope.subscription_id === "blocked",
						);
						return { envelopes, late };
					}),
				),
			);

			expect(output.envelopes.map((envelope) => envelope.kind)).toEqual([
				"subscription.started",
				"thread.list.snapshot",
				"subscription.stopped",
			]);
			expect(Option.getOrUndefined(output.late)).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("orders an in-flight live projection update entirely before stopped", async () => {
		const database_path = await MakeDatabasePath();
		const publication_reached = await Effect.runPromise(Deferred.make<void>());
		const signal_remaining = await Effect.runPromise(Ref.make(0));
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: {
				heartbeat_interval_ms: 60_000,
				heartbeat_timeout_ms: 60_000,
			},
			runtime_metadata: MakeSignalledMetadata(signal_remaining, publication_reached),
		});

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const journal = yield* JournalStore;
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;
						yield* Bootstrap(connection);
						yield* connection.Receive(MakeSubscribe("subscribe-live", "live"));
						yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.started" &&
								envelope.subscription_id === "live",
						);
						yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "thread.list.snapshot" &&
								envelope.subscription_id === "live",
						);

						yield* Ref.set(signal_remaining, 1);
						yield* journal.AcceptThreadCreate({
							kind: "command",
							message_id: "create-live-thread",
							origin: "frontend",
							payload: { title: "Live publication", type: "thread.create" },
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-08-15T08:00:00.000Z",
							thread_id: "live-thread",
						});
						yield* Deferred.await(publication_reached);
						yield* connection.Receive(MakeUnsubscribe("unsubscribe-live", "live"));

						const event = yield* ReceiveUntil(
							connection,
							(envelope) => envelope.kind === "event",
						);
						const update = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "thread.list.upsert" &&
								envelope.subscription_id === "live",
						);
						const stopped = yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.stopped" &&
								envelope.correlation_id === "unsubscribe-live",
						);
						const late = yield* ReceiveMatchingWithin(
							connection,
							(envelope) =>
								"subscription_id" in envelope &&
								envelope.subscription_id === "live",
						);
						return { event, late, stopped, update };
					}),
				),
			);

			expect(output.event).toMatchObject({ kind: "event", thread_id: "live-thread" });
			expect(output.update).toMatchObject({
				kind: "thread.list.upsert",
				subscription_id: "live",
			});
			expect(output.stopped).toMatchObject({
				correlation_id: "unsubscribe-live",
				kind: "subscription.stopped",
			});
			expect(Option.getOrUndefined(output.late)).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("does not resurrect project catalog ownership from queued updates", async () => {
		const database_path = await MakeDatabasePath();
		const second_update_reached = await Effect.runPromise(Deferred.make<void>());
		const signal_remaining = await Effect.runPromise(Ref.make(0));
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			protocol: {
				heartbeat_interval_ms: 60_000,
				heartbeat_timeout_ms: 60_000,
			},
			runtime_metadata: MakeSignalledMetadata(signal_remaining, second_update_reached),
		});

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const project_catalog = yield* ProjectCatalog;
						const connection = yield* server.Open;
						yield* Bootstrap(connection);
						yield* connection.Receive(
							MakeSubscribe("subscribe-projects", "projects", "project.list"),
						);
						yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "subscription.started" &&
								envelope.subscription_id === "projects",
						);
						yield* ReceiveUntil(
							connection,
							(envelope) =>
								envelope.kind === "project.list.snapshot" &&
								envelope.subscription_id === "projects",
						);

						yield* Ref.set(signal_remaining, 1);
						yield* project_catalog.Attach({
							display_name: "First",
							project_id: "project-first",
							root_path: dirname(database_path),
						});
						yield* Deferred.await(second_update_reached);
						yield* project_catalog.Attach({
							display_name: "Second",
							project_id: "project-first",
							root_path: dirname(database_path),
						});
						yield* connection.Receive(
							MakeUnsubscribe("unsubscribe-projects", "projects"),
						);
						const envelopes = [
							yield* ReceiveUntil(
								connection,
								(envelope) =>
									envelope.kind === "project.list.updated" &&
									envelope.subscription_id === "projects",
							),
							yield* ReceiveUntil(
								connection,
								(envelope) =>
									envelope.kind === "subscription.stopped" &&
									envelope.correlation_id === "unsubscribe-projects",
							),
						];

						const late = yield* ReceiveMatchingWithin(
							connection,
							(envelope) =>
								"subscription_id" in envelope &&
								envelope.subscription_id === "projects",
						);
						return { envelopes, late };
					}),
				),
			);

			expect(output.envelopes.map((envelope) => envelope.kind)).toEqual([
				"project.list.updated",
				"subscription.stopped",
			]);
			expect(Option.getOrUndefined(output.late)).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps a reused id owned by its newer claim while the old generation finalizes", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const state = yield* Ref.make<ConnectionState>(MakeReady());
				const handlers = yield* MakeHandlers(state);
				const first = yield* handlers.ClaimPending("shared", "subscribe-old");
				if (first === undefined || "_tag" in first)
					return yield* Effect.die("first claim was not installed");
				expect(yield* handlers.CancelPending("shared")).toBe(true);
				const second = yield* handlers.ClaimPending("shared", "subscribe-new");
				expect(second).toBeDefined();
				yield* handlers.FinalizePending("shared", first);
				return {
					cancelled: yield* Deferred.isDone(first.cancelled),
					state: yield* Ref.get(state),
				};
			}),
		);

		expect(result.cancelled).toBe(true);
		expect(result.state).toMatchObject({
			_tag: "Ready",
			pending_subscriptions: { shared: { message_id: "subscribe-new" } },
		});
	});

	it("removes the current active generation atomically without restoring newer state", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const state = yield* Ref.make<ConnectionState>(MakeReady());
				const handlers = yield* MakeHandlers(state);
				const claim = yield* handlers.ClaimPending("active", "subscribe-active");
				if (claim === undefined || "_tag" in claim)
					return yield* Effect.die("claim was not installed");
				const other_claim = yield* handlers.ClaimPending("other", "subscribe-other");
				if (other_claim === undefined || "_tag" in other_claim)
					return yield* Effect.die("concurrent claim was not installed");
				const pending_heartbeat = {
					deadline_ms: 200,
					message_id: "heartbeat",
					nonce: "nonce",
				};
				const other_subscription = {
					_tag: "project.list" as const,
					sequence: 7,
					stream_id: "projects",
				};
				yield* Ref.update(state, (latest): ConnectionState => {
					if (latest._tag !== "Ready") return latest;
					return {
						...latest,
						last_activity_ms: 99,
						pending_heartbeat,
						pending_subscriptions: {},
						subscription_claims: { active: claim, other: other_claim },
						subscriptions: {
							active: { _tag: "thread.list", sequence: 0, stream_id: "stream" },
							other: other_subscription,
						},
					};
				});
				const cancelled = yield* handlers.CancelPending("active");
				return { cancelled, state: yield* Ref.get(state) };
			}),
		);

		expect(result.cancelled).toBe(true);
		expect(result.state).toMatchObject({
			_tag: "Ready",
			last_activity_ms: 99,
			pending_heartbeat: {
				deadline_ms: 200,
				message_id: "heartbeat",
				nonce: "nonce",
			},
			subscription_claims: { other: { message_id: "subscribe-other" } },
			subscriptions: {
				other: { _tag: "project.list", sequence: 7, stream_id: "projects" },
			},
		});
	});
});
