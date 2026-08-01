import { afterEach, describe, expect, it } from "vitest";

import {
	AdoptForgeEndpoint,
	DecodeLoopbackForgeEndpoint,
	ForgeEndpointStore,
	ForgeEndpointStoreLive,
	ForgeHttpUrl,
	ResolveForgeEndpoint,
} from "../../modules/frontend/src/lib/runtime/forge-endpoint";
import { Effect } from "effect";

const storage = new Map<string, string>();

const install_session_storage = () => {
	(globalThis as { sessionStorage?: unknown }).sessionStorage = {
		getItem: (key: string) => storage.get(key) ?? null,
		removeItem: (key: string) => storage.delete(key),
		setItem: (key: string, value: string) => storage.set(key, value),
	};
};

const Run = <A, E>(program: Effect.Effect<A, E, ForgeEndpointStore>) =>
	Effect.runPromise(program.pipe(Effect.provide(ForgeEndpointStoreLive)));

describe("adopted Forge endpoint", () => {
	afterEach(() => {
		storage.clear();
		delete (globalThis as { sessionStorage?: unknown }).sessionStorage;
	});

	it("accepts only uncredentialed loopback HTTP origins with explicit ports", () => {
		expect(DecodeLoopbackForgeEndpoint("http://127.0.0.1:52985")).toBe(
			"http://127.0.0.1:52985",
		);
		expect(DecodeLoopbackForgeEndpoint("http://127.0.0.1:52985/")).toBe(
			"http://127.0.0.1:52985",
		);
		expect(DecodeLoopbackForgeEndpoint("http://[::1]:52985/")).toBe("http://[::1]:52985");
		expect(DecodeLoopbackForgeEndpoint("http://127.0.0.1")).toBeUndefined();
		expect(DecodeLoopbackForgeEndpoint("https://127.0.0.1:52985")).toBeUndefined();
		expect(DecodeLoopbackForgeEndpoint("http://localhost:52985")).toBeUndefined();
		expect(DecodeLoopbackForgeEndpoint("http://user@127.0.0.1:52985")).toBeUndefined();
		expect(DecodeLoopbackForgeEndpoint("http://attacker.example:80")).toBeUndefined();
		expect(DecodeLoopbackForgeEndpoint("not a url")).toBeUndefined();
		expect(DecodeLoopbackForgeEndpoint(42)).toBeUndefined();
	});

	it("adopts a session-scoped endpoint only from a non-HTTP(S) page", async () => {
		install_session_storage();

		expect(await Run(AdoptForgeEndpoint("http://127.0.0.1:52985/", "http:"))).toBe(false);
		expect(await Run(AdoptForgeEndpoint("http://127.0.0.1:52985/", "https:"))).toBe(false);
		expect(await Run(ResolveForgeEndpoint)).toBeUndefined();
		expect(await Run(ForgeHttpUrl("/health"))).toBe("/health");

		expect(await Run(AdoptForgeEndpoint("http://127.0.0.1:52985/", "artisan:"))).toBe(true);
		expect(await Run(ResolveForgeEndpoint)).toBe("http://127.0.0.1:52985");
		expect(await Run(ForgeHttpUrl("/health"))).toBe("http://127.0.0.1:52985/health");
		expect(await Run(ForgeHttpUrl("/api/instances"))).toBe(
			"http://127.0.0.1:52985/api/instances",
		);
	});

	it("ignores a tampered stored endpoint instead of trusting it", async () => {
		install_session_storage();
		storage.set("artisan.forge-endpoint", "http://attacker.example:80");

		expect(await Run(ResolveForgeEndpoint)).toBeUndefined();
		expect(await Run(ForgeHttpUrl("/health"))).toBe("/health");
	});

	it("stays same-origin without any storage in the environment", async () => {
		expect(await Run(AdoptForgeEndpoint("http://127.0.0.1:52985/", "artisan:"))).toBe(true);
		expect(await Run(ResolveForgeEndpoint)).toBeUndefined();
		expect(await Run(ForgeHttpUrl("/health"))).toBe("/health");
	});
});
