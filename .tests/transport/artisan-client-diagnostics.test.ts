import { Effect, Layer, ManagedRuntime } from "effect";
import { describe, expect, it } from "vitest";

import {
	ArtisanClient,
	make_artisan_client_layer,
	MessagePortConnector,
	MessagePortConnectorError,
	TransportRuntimeLive,
	type TransportDiagnosticEvent,
} from "@artisan/transport/client";

import { make_transport_test_harness } from "./message-channel-harness";

const events_of_kind = <Kind extends TransportDiagnosticEvent["kind"]>(
	events: ReadonlyArray<TransportDiagnosticEvent>,
	kind: Kind,
) =>
	events.filter(
		(event): event is Extract<TransportDiagnosticEvent, { kind: Kind }> => event.kind === kind,
	);

describe("Artisan client transport diagnostics", () => {
	it("journals a healthy session, its death, and the resumed replacement", async () => {
		const harness = await make_transport_test_harness();

		try {
			await Effect.runPromise(harness.client.ListThreads.pipe(Effect.timeout("2 seconds")));
			harness.close_current_connection();
			const snapshot = await Effect.runPromise(
				Effect.gen(function* () {
					while (true) {
						const current = yield* harness.client.Diagnostics;
						if (
							events_of_kind(current.events, "session.ready").length >= 2 &&
							events_of_kind(current.events, "session.ended").length >= 1
						) {
							return current;
						}
						yield* Effect.sleep("5 millis");
					}
				}).pipe(Effect.timeout("2 seconds")),
			);

			const attempts = events_of_kind(snapshot.events, "session.attempt");
			const negotiations = events_of_kind(snapshot.events, "session.negotiating");
			const endings = events_of_kind(snapshot.events, "session.ended");

			expect(attempts[0]?.ordinal).toBe(1);
			expect(attempts.at(-1)?.ordinal).toBe(attempts.length);
			expect(negotiations[0]?.resume_mode).toBe("fresh");
			expect(negotiations.at(-1)?.resume_mode).toBe("resume");
			expect(endings[0]?.reached_ready).toBe(true);
			expect(endings[0]?.code).toBe("connection");
			expect(endings[0]?.lifetime_ms).toBeGreaterThanOrEqual(0);
		} finally {
			await harness.dispose();
		}
	});

	it("journals every failed attempt, exhaustion, and the released retry", async () => {
		const connector = Layer.succeed(MessagePortConnector, {
			Connect: Effect.fail(
				new MessagePortConnectorError({ cause: new Error("Forge unavailable") }),
			),
		});
		const runtime = ManagedRuntime.make(
			make_artisan_client_layer({ reconnect_delay_ms: 1 }).pipe(
				Layer.provide(connector),
				Layer.provide(TransportRuntimeLive),
			),
		);

		try {
			const client = await runtime.runPromise(ArtisanClient);
			await runtime.runPromise(
				Effect.gen(function* () {
					while ((yield* client.ConnectionState).phase !== "exhausted") {
						yield* Effect.sleep("1 millis");
					}
				}).pipe(Effect.timeout("2 seconds")),
			);
			const snapshot = await runtime.runPromise(client.Diagnostics);
			const attempts = events_of_kind(snapshot.events, "session.attempt");
			const endings = events_of_kind(snapshot.events, "session.ended");
			const exhaustions = events_of_kind(snapshot.events, "supervisor.exhausted");

			expect(attempts.map((event) => event.ordinal)).toEqual([1, 2, 3, 4, 5]);
			expect(endings).toHaveLength(5);
			expect(endings.every((event) => !event.reached_ready)).toBe(true);
			expect(endings.every((event) => event.code === "connection")).toBe(true);
			expect(exhaustions[0]?.attempts).toBe(5);

			await runtime.runPromise(client.RetryConnection);
			const retried = await runtime.runPromise(client.Diagnostics);
			expect(events_of_kind(retried.events, "supervisor.retry_released")).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});
});
