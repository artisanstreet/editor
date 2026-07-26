import { describe, expect, it } from "vitest";

import { ResolveWebSocketRuntimeTarget } from "../../modules/frontend/src/lib/runtime/websocket-runtime";

describe("frontend WebSocket runtime target", () => {
	it("uses an explicit development endpoint before browser discovery", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				development_url: "http://127.0.0.1:8787/socket",
				is_development: true,
				location: { origin: "http://localhost:5173", protocol: "http:" },
			}),
		).toEqual({ _tag: "websocket", url: "ws://127.0.0.1:8787/socket" });
	});

	it("derives the colocated Forge endpoint for an ordinary browser page", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				is_development: false,
				location: { origin: "https://artisan.example", protocol: "https:" },
			}),
		).toEqual({ _tag: "websocket", url: "wss://artisan.example/api/ws" });
	});

	it("prefers a desktop-owned WebSocket endpoint when one is exposed", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				desktop: { forgeWebSocketEndpoint: "ws://127.0.0.1:4311/api/ws" },
				is_development: false,
				location: { origin: "app://artisan", protocol: "app:" },
			}),
		).toEqual({ _tag: "websocket", url: "ws://127.0.0.1:4311/api/ws" });
	});

	it("retains the MessagePort path for installed desktop builds without a socket endpoint", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				desktop: {},
				is_development: false,
				location: { origin: "app://artisan", protocol: "app:" },
			}),
		).toEqual({ _tag: "desktop" });
	});
});
