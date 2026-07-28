import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import {
	InstanceCardPath,
	ListForgeInstances,
	ResolveInstanceRegistryRoot,
	WriteForgeState,
} from "../../modules/forge/src/index";

const run = <A, E>(effect: Effect.Effect<A, E, never>) => Effect.runPromise(effect);

const card = (overrides: Partial<Parameters<typeof WriteForgeState>[1]>) => ({
	endpoint: "http://127.0.0.1:4849/",
	instance_id: "forge_live",
	pid: process.pid,
	started_at: "2026-07-28T00:00:00.000Z",
	version: 1 as const,
	...overrides,
});

describe("forge instance registry", () => {
	it("resolves the machine-global root from platform environment", () => {
		expect(ResolveInstanceRegistryRoot({ LOCALAPPDATA: "C:/Users/s/AppData/Local" })).toBe(
			join("C:/Users/s/AppData/Local", "Artisan"),
		);
		expect(ResolveInstanceRegistryRoot({})).toBeUndefined();
	});

	it("lists live cards and drops instances whose process is gone", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-instances-"));
		const writes = Effect.gen(function* () {
			yield* WriteForgeState(InstanceCardPath(root, "forge_live"), card({}));
			yield* WriteForgeState(
				InstanceCardPath(root, "forge_dead"),
				card({ instance_id: "forge_dead", pid: 0x7fffffff }),
			);
		}).pipe(Effect.provide(Layer.orDie(MakeSnowflakeIdLive(9))));
		await run(writes as Effect.Effect<void, never, never>);

		const instances = await run(ListForgeInstances(root));
		expect(instances.map((instance) => instance.instance_id)).toEqual(["forge_live"]);
	});

	it("discards cards whose endpoint is not plain-HTTP loopback", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-instances-endpoints-"));
		const writes = Effect.gen(function* () {
			yield* WriteForgeState(
				InstanceCardPath(root, "forge_external"),
				card({ endpoint: "https://attacker.invalid/", instance_id: "forge_external" }),
			);
			yield* WriteForgeState(
				InstanceCardPath(root, "forge_script"),
				card({ endpoint: "javascript:alert(1)", instance_id: "forge_script" }),
			);
			yield* WriteForgeState(InstanceCardPath(root, "forge_live"), card({}));
		});
		await run(writes as Effect.Effect<void, never, never>);

		const instances = await run(ListForgeInstances(root));
		expect(instances.map((instance) => instance.instance_id)).toEqual(["forge_live"]);
	});

	it("returns an empty listing for a root that does not exist", async () => {
		const instances = await run(
			ListForgeInstances(join(tmpdir(), "artisan-instances-missing")),
		);
		expect(instances).toEqual([]);
	});
});
