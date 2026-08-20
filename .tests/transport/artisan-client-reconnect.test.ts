import { Effect, Stream } from "effect";
import { describe, expect, it } from "vitest";

import type { TransportDiagnosticEvent } from "@artisan/transport/client";

import { make_transport_test_harness, wait_for } from "./message-channel-harness";

const events_of_kind = <Kind extends TransportDiagnosticEvent["kind"]>(
	events: ReadonlyArray<TransportDiagnosticEvent>,
	kind: Kind,
) =>
	events.filter(
		(event): event is Extract<TransportDiagnosticEvent, { kind: Kind }> => event.kind === kind,
	);

const await_phase = (
	client: Awaited<ReturnType<typeof make_transport_test_harness>>["client"],
	phase: "ready" | "exhausted",
) =>
	Effect.gen(function* () {
		while ((yield* client.ConnectionState).phase !== phase) {
			yield* Effect.sleep("5 millis");
		}
	}).pipe(Effect.timeout("5 seconds"));

describe("Artisan client reconnect policy", () => {
	it("does not spend the reconnect budget on sessions that reached ready", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_attempts: 2, reconnect_delay_ms: 5 },
		});

		try {
			await Effect.runPromise(harness.client.ListThreads.pipe(Effect.timeout("2 seconds")));

			/**
			 * Three healthy-session deaths exceed a budget of two, so this only
			 * survives if readiness restores the budget instead of every death
			 * counting toward one lifetime quota.
			 */
			for (let kill = 1; kill <= 3; kill += 1) {
				harness.close_current_connection();
				await wait_for(() => harness.connector_snapshot().connections >= kill + 1);
				await Effect.runPromise(await_phase(harness.client, "ready"));
			}

			const snapshot = await Effect.runPromise(harness.client.Diagnostics);

			expect(events_of_kind(snapshot.events, "supervisor.exhausted")).toHaveLength(0);
			expect(events_of_kind(snapshot.events, "session.ready").length).toBeGreaterThanOrEqual(
				4,
			);
		} finally {
			await harness.dispose();
		}
	});

	it("reconnects a session whose inbound frames silently stop", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			protocol: { heartbeat_interval_ms: 50, heartbeat_timeout_ms: 150 },
		});

		try {
			await Effect.runPromise(harness.client.ListThreads.pipe(Effect.timeout("2 seconds")));

			/**
			 * A zombie socket delivers nothing and never fires a close event —
			 * the shape a host suspend or a killed backend leaves behind. Only
			 * the inbound-liveness watchdog can notice, by the advertised
			 * heartbeat cadence going silent, and hand the session to the
			 * supervisor for a replacement connection.
			 */
			harness.mute_current_connection();
			await wait_for(() => harness.connector_snapshot().connections >= 2, 5_000);
			await Effect.runPromise(await_phase(harness.client, "ready"));
			await Effect.runPromise(harness.client.ListThreads.pipe(Effect.timeout("2 seconds")));

			const snapshot = await Effect.runPromise(harness.client.Diagnostics);
			const silence_endings = events_of_kind(snapshot.events, "session.ended").filter(
				(event) => event.message.includes("silent past its heartbeat window"),
			);

			expect(silence_endings.length).toBeGreaterThanOrEqual(1);
			expect(events_of_kind(snapshot.events, "supervisor.exhausted")).toHaveLength(0);
		} finally {
			await harness.dispose();
		}
	});

	it("survives a session that dies inside its readiness window", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
			protocol: { subscription_started_delay_ms: { "orchestration.graph": 250 } },
		});

		try {
			await Effect.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const thread_updates = yield* harness.client.SubscribeThreadList;
						const graph_updates =
							yield* harness.client.SubscribeOrchestrationGraph("group_mid");

						yield* thread_updates.pipe(
							Stream.take(1),
							Stream.runCollect,
							Effect.forkScoped,
						);
						yield* graph_updates.pipe(
							Stream.take(1),
							Stream.runCollect,
							Effect.forkScoped,
						);

						/** Both subscriptions are started; kill the healthy session. */
						yield* Effect.sync(harness.close_current_connection);
						yield* Effect.promise(() =>
							wait_for(() => harness.connector_snapshot().connections >= 2),
						);

						/**
						 * The replacement resends both subscribes; thread.list
						 * starts within milliseconds while the graph's start is
						 * held for 250ms. Killing here reproduces a mid-readiness
						 * death with one subscription already started — the state
						 * that once leaked into the next resume and made its
						 * resubscribe die as a duplicate within milliseconds.
						 */
						yield* Effect.promise(() =>
							wait_for(() => harness.protocol_snapshot().subscriptions.length >= 4),
						);
						yield* Effect.sleep("30 millis");
						yield* Effect.sync(harness.close_current_connection);

						yield* await_phase(harness.client, "ready");
					}),
				).pipe(Effect.timeout("10 seconds")),
			);

			const snapshot = await Effect.runPromise(harness.client.Diagnostics);
			const conflicted = events_of_kind(snapshot.events, "session.ended").filter(
				(event) => event.code === "correlation_conflict",
			);

			expect(conflicted).toHaveLength(0);
			expect(events_of_kind(snapshot.events, "supervisor.exhausted")).toHaveLength(0);
		} finally {
			await harness.dispose();
		}
	});

	it("drops a poisoned resume position and re-bootstraps fresh", async () => {
		const harness = await make_transport_test_harness({
			client: { reconnect_delay_ms: 5 },
		});

		try {
			/** Advance the client's durable cursors past what the wipe will keep. */
			await Effect.runPromise(
				Effect.gen(function* () {
					const graph = yield* harness.client.GetOrchestrationGraph("group_wipe");

					yield* harness.client.Command({
						command_id: "rename_before_wipe",
						payload: {
							agent_id: graph.group.coordinator_agent_id,
							display_name: "Wiped",
							group_id: "group_wipe",
							type: "agent_instance.rename",
						},
						thread_id: graph.group.thread_id,
					});

					while ((yield* harness.client.Cursors).last_journal_sequence < 1) {
						yield* Effect.sleep("5 millis");
					}
				}).pipe(Effect.timeout("2 seconds")),
			);

			await Effect.runPromise(harness.wipe_journal);
			harness.close_current_connection();

			/**
			 * The resume replays into a journal that regressed behind the
			 * client's cursors. Without dropping the resume position, every
			 * later attempt — including a manual retry — would fail identically.
			 */
			await Effect.runPromise(
				Effect.gen(function* () {
					while (true) {
						const current = yield* harness.client.Diagnostics;
						/**
						 * The connection phase lags a recycling supervisor, so
						 * recovery is observed through the journal: the drop
						 * entry, then a negotiation that really went out fresh.
						 */
						if (
							events_of_kind(current.events, "session.resume_dropped").length >= 1 &&
							events_of_kind(current.events, "session.negotiating").at(-1)
								?.resume_mode === "fresh"
						) {
							return;
						}
						yield* Effect.sleep("5 millis");
					}
				}).pipe(Effect.timeout("5 seconds")),
			);
			await Effect.runPromise(await_phase(harness.client, "ready"));

			const snapshot = await Effect.runPromise(harness.client.Diagnostics);
			const dropped = events_of_kind(snapshot.events, "session.resume_dropped");
			const negotiations = events_of_kind(snapshot.events, "session.negotiating");

			expect(dropped[0]?.code).toBe("stream_gap");
			expect(negotiations.at(-1)?.resume_mode).toBe("fresh");
			await Effect.runPromise(harness.client.ListThreads.pipe(Effect.timeout("2 seconds")));
		} finally {
			await harness.dispose();
		}
	});
});
