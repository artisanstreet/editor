import { mkdtemp, mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { Effect, ManagedRuntime } from "effect";

import { make_node_instance_store_layer } from "../../modules/cli/src/node-instance-store";
import { ForgeInstanceStore } from "../../modules/cli/src/instance";

const MakeConfig = (data_root: string) => ({
	data_root,
	listen_host: "127.0.0.1" as const,
	listen_port: 0,
	mode: "local" as const,
	version: 1 as const,
});

const WithStore = async (
	operation: (store: ForgeInstanceStore["Service"], home: string) => Promise<void>,
) => {
	const home = await mkdtemp(join(tmpdir(), "artisan-instance-store-"));
	const runtime = ManagedRuntime.make(make_node_instance_store_layer(home));
	try {
		await operation(await runtime.runPromise(ForgeInstanceStore), home);
	} finally {
		await runtime.dispose();
		await rm(home, { force: true, recursive: true });
	}
};

describe("node Forge instance store", () => {
	it("writes schema-valid atomic JSON at the home root and persists a 32-byte token", async () => {
		await WithStore(async (store, home) => {
			const data_root = join(home, "data");
			await mkdir(data_root);
			const config = MakeConfig(data_root);

			await Effect.runPromise(store.Ensure(config));
			const paths = await Effect.runPromise(store.Paths());
			expect(paths.config_path).toBe(join(home, "config.json"));
			expect(JSON.parse(await readFile(paths.config_path, "utf8"))).toEqual(
				expect.objectContaining({ version: 1 }),
			);
			const first_token = (await Effect.runPromise(store.LoadSecrets())).auth_token;
			expect(first_token).toMatch(/^[A-Za-z0-9_-]{43}$/);
			await Effect.runPromise(store.Ensure(config));
			expect((await Effect.runPromise(store.LoadSecrets())).auth_token).toBe(first_token);
		});
	});

	it("treats malformed state as absent and preserves nonmatching owned state", async () => {
		await WithStore(async (store, home) => {
			const data_root = join(home, "data");
			await mkdir(data_root);
			await Effect.runPromise(store.Ensure(MakeConfig(data_root)));
			const paths = await Effect.runPromise(store.Paths());
			await writeFile(paths.state_path, "not-json", "utf8");
			expect(await Effect.runPromise(store.ReadState())).toBeUndefined();
			await writeFile(
				paths.state_path,
				JSON.stringify({
					endpoint: "http://127.0.0.1:4848",
					instance_id: "2ef3d1c0-e8a4-4f4d-9d8a-744b1f18879d",
					pid: 42,
					started_at: "2026-07-26T00:00:00.000Z",
					version: 1,
				}),
				"utf8",
			);
			expect(
				await Effect.runPromise(
					store.RemoveStateIfOwned("152f50de-381a-48db-b52f-6fa5bf4166d7"),
				),
			).toBe(false);
			expect(await Effect.runPromise(store.ReadState())).toMatchObject({ pid: 42 });
			expect(
				await Effect.runPromise(
					store.RemoveStateIfOwned("2ef3d1c0-e8a4-4f4d-9d8a-744b1f18879d"),
				),
			).toBe(true);
			expect(await Effect.runPromise(store.ReadState())).toBeUndefined();
		});
	});

	it("migrates a single legacy profile directory into the home root", async () => {
		await WithStore(async (store, home) => {
			const legacy = join(home, "profiles", "browser-dev");
			await mkdir(join(legacy, "data"), { recursive: true });
			await writeFile(
				join(legacy, "config.json"),
				`${JSON.stringify(MakeConfig(join(legacy, "data")))}\n`,
				"utf8",
			);
			await writeFile(
				join(legacy, "secrets.json"),
				`${JSON.stringify({ auth_token: "a".repeat(43), version: 1 })}\n`,
				"utf8",
			);
			await writeFile(join(legacy, "forge.log"), "log line\n", "utf8");

			const paths = await Effect.runPromise(store.Paths());

			expect(paths.config_path).toBe(join(home, "config.json"));
			expect(JSON.parse(await readFile(join(home, "config.json"), "utf8"))).toMatchObject({
				version: 1,
			});
			expect(await readFile(join(home, "forge.log"), "utf8")).toBe("log line\n");
			expect((await readdir(join(home, "data"), { recursive: false })).length).toBe(0);
			await expect(readdir(join(home, "profiles"))).rejects.toMatchObject({
				code: "ENOENT",
			});
			expect((await Effect.runPromise(store.LoadSecrets())).auth_token).toBe("a".repeat(43));
		});
	});

	it("refuses to migrate a home with multiple legacy profiles", async () => {
		await WithStore(async (store, home) => {
			await mkdir(join(home, "profiles", "default"), { recursive: true });
			await mkdir(join(home, "profiles", "team-1"), { recursive: true });

			await expect(Effect.runPromise(store.Load())).rejects.toMatchObject({
				code: "legacy_profiles",
			});
			/** Nothing moved: both legacy directories stay for the user to resolve. */
			expect((await readdir(join(home, "profiles"))).toSorted()).toEqual([
				"default",
				"team-1",
			]);
		});
	});

	it("ignores leftover legacy directories once the home root is configured", async () => {
		await WithStore(async (store, home) => {
			const data_root = join(home, "data");
			await mkdir(data_root);
			await Effect.runPromise(store.Ensure(MakeConfig(data_root)));
			await mkdir(join(home, "profiles", "default"), { recursive: true });
			await mkdir(join(home, "profiles", "team-1"), { recursive: true });

			expect(await Effect.runPromise(store.Load())).toMatchObject({ version: 1 });
		});
	});
});
