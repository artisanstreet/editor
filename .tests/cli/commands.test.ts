import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { Effect, Layer, ManagedRuntime, Stream } from "effect";

import {
	ForgeAutostart,
	ForgeTaskName,
	task_scheduler_arguments,
} from "../../modules/cli/src/adapters";
import { ForgeLifecycle } from "../../modules/cli/src/lifecycle";
import { ForgeOperations, make_forge_operations_layer } from "../../modules/cli/src/operations";
import { make_node_profile_store_layer } from "../../modules/cli/src/node-profile-store";
import { ForgeProfileStore } from "../../modules/cli/src/profile";

const MakeRuntime = (home: string) => {
	const profiles = make_node_profile_store_layer(home);
	const lifecycle = Layer.succeed(
		ForgeLifecycle,
		ForgeLifecycle.of({
			Doctor: () => Effect.succeed({ healthy: false, state: "missing" as const }),
			Open: () => Effect.die("not used"),
			Restart: () => Effect.void,
			Start: () => Effect.void,
			Status: () => Effect.succeed({ state: "missing" as const }),
			Stop: () => Effect.void,
		}),
	);
	const autostart = Layer.succeed(
		ForgeAutostart,
		ForgeAutostart.of({
			Configure: () => Effect.succeed({ state: "unsupported" as const }),
			Status: () => Effect.succeed({ state: "unsupported" as const }),
		}),
	);
	return ManagedRuntime.make(
		Layer.mergeAll(
			profiles,
			make_forge_operations_layer.pipe(
				Layer.provide(Layer.mergeAll(profiles, lifecycle, autostart)),
			),
		),
	);
};

describe("ae command operations", () => {
	it("reads bounded log tails and follows UTF-8 log content interruptibly", async () => {
		const home = await mkdtemp(join(tmpdir(), "artisan-cli-"));
		const runtime = MakeRuntime(home);
		try {
			const profiles = await runtime.runPromise(ForgeProfileStore);
			await mkdir(join(home, "data"));
			await runtime.runPromise(
				profiles.Ensure("default", {
					data_root: join(home, "data"),
					listen_host: "127.0.0.1",
					listen_port: 0,
					mode: "local",
					project_roots: [home],
					version: 1,
				}),
			);
			const paths = await runtime.runPromise(profiles.Paths("default"));
			await writeFile(paths.log_path, "first\nsecond\nthird \u00e6\n", "utf8");
			const operations = await runtime.runPromise(ForgeOperations);
			expect(await runtime.runPromise(operations.ReadLogs("default", 2))).toEqual([
				"second",
				"third \u00e6",
			]);
			setTimeout(
				() =>
					void writeFile(
						paths.log_path,
						"first\nsecond\nthird \u00e6\nfourth \u00f8\n",
						"utf8",
					),
				50,
			);
			const followed = await runtime.runPromise(
				operations.FollowLogs("default").pipe(Stream.take(1), Stream.runCollect),
			);
			expect([...followed]).toEqual(["fourth \u00f8\n"]);
		} finally {
			await runtime.dispose();
			await rm(home, { force: true, recursive: true });
		}
	});

	it("returns deterministic secret-free doctor checks", async () => {
		const home = await mkdtemp(join(tmpdir(), "artisan-cli-"));
		const runtime = MakeRuntime(home);
		try {
			const profiles = await runtime.runPromise(ForgeProfileStore);
			await mkdir(join(home, "data"));
			await runtime.runPromise(
				profiles.Ensure("default", {
					data_root: join(home, "data"),
					listen_host: "127.0.0.1",
					listen_port: 0,
					mode: "headless",
					project_roots: [home],
					version: 1,
				}),
			);
			const secret = await runtime.runPromise(profiles.LoadSecrets("default"));
			const report = await runtime.runPromise(
				(await runtime.runPromise(ForgeOperations)).Doctor("default"),
			);
			expect(report.checks.map((check) => check.name)).toEqual([
				"profile",
				"config",
				"roots",
				"artifacts",
				"codex",
				"autostart",
				"live",
			]);
			expect(JSON.stringify(report)).not.toContain(secret.auth_token);
		} finally {
			await runtime.dispose();
			await rm(home, { force: true, recursive: true });
		}
	});

	it("keeps scheduler arguments profile-scoped", () => {
		expect(ForgeTaskName("default")).toBe("Artisan Forge default");
		const arguments_without_entry = task_scheduler_arguments({
			enabled: true,
			executable_path: "C:/Program Files/Artisan/ae.exe",
			profile: "default",
		});
		expect(arguments_without_entry).toEqual(
			expect.arrayContaining(["/RL", "LIMITED", "/SC", "ONLOGON"]),
		);
		const arguments_with_entry = task_scheduler_arguments({
			cli_entry_path: "C:/Program Files/Artisan/ae.js",
			enabled: true,
			executable_path: "C:/Program Files/Artisan/node.exe",
			profile: "default",
		});
		expect(arguments_with_entry[arguments_with_entry.indexOf("/TR") + 1]).toBe(
			'"C:/Program Files/Artisan/node.exe" "C:/Program Files/Artisan/ae.js" start --profile default',
		);
		expect(
			task_scheduler_arguments({
				enabled: true,
				executable_path: "C:/Program Files/Artisan/ae.exe",
				profile: "default",
			}),
		).toEqual(arguments_without_entry);
	});
});
