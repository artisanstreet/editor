import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { Effect, ManagedRuntime } from "effect";

import { make_node_profile_store_layer } from "../../modules/cli/src/node-profile-store";
import { ForgeProfileStore } from "../../modules/cli/src/profile";

const MakeConfig = (data_root: string) => ({
	data_root,
	listen_host: "127.0.0.1" as const,
	listen_port: 0,
	mode: "local" as const,
	version: 1 as const,
});

const WithStore = async (
	operation: (store: ForgeProfileStore["Service"], home: string) => Promise<void>,
) => {
	const home = await mkdtemp(join(tmpdir(), "artisan-profile-store-"));
	const runtime = ManagedRuntime.make(make_node_profile_store_layer(home));
	try {
		await operation(await runtime.runPromise(ForgeProfileStore), home);
	} finally {
		await runtime.dispose();
		await rm(home, { force: true, recursive: true });
	}
};

describe("node Forge profile store", () => {
	it("writes schema-valid atomic JSON and persists a 32-byte token", async () => {
		await WithStore(async (store, home) => {
			const data_root = join(home, "data");
			await mkdir(data_root);
			const config = MakeConfig(data_root);

			await Effect.runPromise(store.Ensure("default", config));
			const paths = await Effect.runPromise(store.Paths("default"));
			expect(JSON.parse(await readFile(paths.config_path, "utf8"))).toEqual(
				expect.objectContaining({ version: 1 }),
			);
			const first_token = (await Effect.runPromise(store.LoadSecrets("default"))).auth_token;
			expect(first_token).toMatch(/^[A-Za-z0-9_-]{43}$/);
			await Effect.runPromise(store.Ensure("default", config));
			expect((await Effect.runPromise(store.LoadSecrets("default"))).auth_token).toBe(
				first_token,
			);
		});
	});

	it("rejects unsafe profile names", async () => {
		await WithStore(async (store, home) => {
			const existing = join(home, "existing");
			await mkdir(existing);
			await expect(Effect.runPromise(store.Paths("../escape"))).rejects.toMatchObject({
				code: "invalid",
			});
			await expect(
				Effect.runPromise(store.Ensure("CON", MakeConfig(existing))),
			).rejects.toMatchObject({ code: "invalid" });
		});
	});

	it("treats malformed state as absent and preserves nonmatching owned state", async () => {
		await WithStore(async (store, home) => {
			const data_root = join(home, "data");
			await mkdir(data_root);
			await Effect.runPromise(store.Ensure("default", MakeConfig(data_root)));
			const paths = await Effect.runPromise(store.Paths("default"));
			await writeFile(paths.state_path, "not-json", "utf8");
			expect(await Effect.runPromise(store.ReadState("default"))).toBeUndefined();
			await writeFile(
				paths.state_path,
				JSON.stringify({
					endpoint: "http://127.0.0.1:4848",
					instance_id: "2ef3d1c0-e8a4-4f4d-9d8a-744b1f18879d",
					pid: 42,
					profile: "other",
					started_at: "2026-07-26T00:00:00.000Z",
					version: 1,
				}),
				"utf8",
			);
			expect(await Effect.runPromise(store.ReadState("default"))).toMatchObject({
				profile: "other",
			});
			expect(
				await Effect.runPromise(
					store.RemoveStateIfOwned("default", "2ef3d1c0-e8a4-4f4d-9d8a-744b1f18879d"),
				),
			).toBe(false);
			await writeFile(
				paths.state_path,
				JSON.stringify({
					endpoint: "http://127.0.0.1:4848",
					instance_id: "2ef3d1c0-e8a4-4f4d-9d8a-744b1f18879d",
					pid: 42,
					profile: "default",
					started_at: "2026-07-26T00:00:00.000Z",
					version: 1,
				}),
				"utf8",
			);
			expect(
				await Effect.runPromise(
					store.RemoveStateIfOwned("default", "152f50de-381a-48db-b52f-6fa5bf4166d7"),
				),
			).toBe(false);
			expect(await Effect.runPromise(store.ReadState("default"))).toMatchObject({ pid: 42 });
		});
	});
});
