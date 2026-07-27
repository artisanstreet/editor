import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { Effect, Layer, ManagedRuntime, Option, Stream } from "effect";

import {
	CliPlatform,
	ForgeAutostart,
	ForgeTaskName,
	task_scheduler_arguments,
} from "../../modules/cli/src/adapters";
import { ForgeLifecycle } from "../../modules/cli/src/lifecycle";
import { ForgeOperations, make_forge_operations_layer } from "../../modules/cli/src/operations";
import { make_node_profile_store_layer } from "../../modules/cli/src/node-profile-store";
import { ForgeProfileStore } from "../../modules/cli/src/profile";
import { ForgeProtocolCommand } from "../../modules/cli/src/protocol-handler";

const MakeRuntime = (home: string) => {
	const platform = Layer.succeed(
		CliPlatform,
		CliPlatform.of({
			cli_entry_path: Option.some("C:\\Artisan\\ae.js"),
			executable_path: "C:\\Artisan\\node.exe",
			home,
			kind: "win32",
		}),
	);
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
			platform,
			make_forge_operations_layer.pipe(
				Layer.provide(Layer.mergeAll(profiles, lifecycle, autostart, platform)),
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

	it("continues doctor diagnostics when the profile is missing", async () => {
		const home = await mkdtemp(join(tmpdir(), "artisan-cli-"));
		const runtime = MakeRuntime(home);
		try {
			const report = await runtime.runPromise(
				(await runtime.runPromise(ForgeOperations)).Doctor("default"),
			);
			expect(report.healthy).toBe(false);
			expect(report.checks.map((check) => check.name)).toEqual([
				"profile",
				"config",
				"artifacts",
				"codex",
				"autostart",
				"live",
			]);
			expect(report.checks[0]).toMatchObject({
				name: "profile",
				state: "error",
			});
		} finally {
			await runtime.dispose();
			await rm(home, { force: true, recursive: true });
		}
	});

	it("does not create a missing profile during Forge repair", async () => {
		const home = await mkdtemp(join(tmpdir(), "artisan-cli-"));
		const runtime = MakeRuntime(home);
		try {
			const operations = await runtime.runPromise(ForgeOperations);
			const report = await runtime.runPromise(operations.Repair("default"));
			const profiles = await runtime.runPromise(ForgeProfileStore);

			await expect(runtime.runPromise(profiles.Load("default"))).rejects.toMatchObject({
				code: "invalid",
			});
			await expect(
				readFile(join(home, "profiles", "default", "config.json")),
			).rejects.toMatchObject({ code: "ENOENT" });
			expect(report.checks.find((check) => check.name === "profile")?.state).toBe("error");
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

	it("builds a quoted desktop protocol command", () => {
		expect(ForgeProtocolCommand("C:\\Program Files\\Artisan Editor\\Artisan Editor.exe")).toBe(
			'"C:\\Program Files\\Artisan Editor\\Artisan Editor.exe" "%1"',
		);
	});
});
