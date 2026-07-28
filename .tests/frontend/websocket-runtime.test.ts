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

	it("targets the adopted loopback Forge from the installed editor's app scheme", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				forge_endpoint: "http://127.0.0.1:52985",
				is_development: false,
				location: { origin: "artisan://app", protocol: "artisan:" },
			}),
		).toEqual({ _tag: "websocket", url: "ws://127.0.0.1:52985/api/ws" });
	});

	it("reports unavailable outside HTTP(S) instead of falling back to a native bridge", () => {
		expect(
			ResolveWebSocketRuntimeTarget({
				is_development: false,
				location: { origin: "app://artisan", protocol: "app:" },
			}),
		).toEqual({ _tag: "unavailable" });
		expect(
			ResolveWebSocketRuntimeTarget({
				forge_endpoint: "not a url",
				is_development: false,
				location: { origin: "artisan://app", protocol: "artisan:" },
			}),
		).toEqual({ _tag: "unavailable" });
	});
});
