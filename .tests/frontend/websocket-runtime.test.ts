import { ArtisanClient } from "@artisan/transport/client";
import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	make_websocket_client_runtime_layer,
	ResolveWebSocketRuntimeTarget,
} from "../../modules/frontend/src/lib/runtime/websocket-runtime";

describe("frontend WebSocket runtime target", () => {
	it("uses an explicit development endpoint before browser discovery", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				development_url: "http://127.0.0.1:8787/socket",
				is_development: true,
				location: { origin: "http://localhost:5173", protocol: "http:", search: "" },
			}),
		).toEqual({ _tag: "websocket", url: "ws://127.0.0.1:8787/socket" });
	});

	it("derives the colocated Forge endpoint for an ordinary browser page", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				is_development: false,
				location: { origin: "https://artisan.example", protocol: "https:", search: "" },
			}),
		).toEqual({ _tag: "websocket", url: "wss://artisan.example/api/ws" });
	});

	it("targets the adopted loopback Forge from the installed editor's app scheme", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				forge_endpoint: "http://127.0.0.1:52985",
				is_development: false,
				location: { origin: "artisan://app", protocol: "artisan:", search: "" },
			}),
		).toEqual({ _tag: "websocket", url: "ws://127.0.0.1:52985/api/ws" });
	});

	it("parks the temporary desktop loader without publishing connection failures", async () => {
		const target = ResolveWebSocketRuntimeTarget({
			is_development: false,
			location: {
				origin: "artisan://app",
				protocol: "artisan:",
				search: "?artisan-loader=1",
			},
		});
		expect(target).toEqual({ _tag: "pending" });

		const snapshot = await Effect.runPromise(
			Effect.gen(function* () {
				const client = yield* ArtisanClient;
				yield* Effect.sleep("20 millis");
				expect(yield* client.ConnectionState).toEqual({ phase: "connecting" });
				return yield* client.Diagnostics;
			}).pipe(Effect.provide(make_websocket_client_runtime_layer(target))),
		);
		const event_kinds = snapshot.events.map((event) => event.kind);
		expect(event_kinds).toContain("session.attempt");
		expect(event_kinds).not.toContain("session.ended");
		expect(event_kinds).not.toContain("supervisor.exhausted");
		expect(event_kinds).not.toContain("error.published");
	});

	it("reports unavailable outside HTTP(S) instead of falling back to a native bridge", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				is_development: false,
				location: { origin: "app://artisan", protocol: "app:", search: "" },
			}),
		).toEqual({ _tag: "unavailable" });
		expect(
			ResolveWebSocketRuntimeTarget({
				forge_endpoint: "not a url",
				is_development: false,
				location: { origin: "artisan://app", protocol: "artisan:", search: "" },
			}),
		).toEqual({ _tag: "unavailable" });
	});
});
