import { describe, expect, it } from "vitest";
import { Effect, Layer, ManagedRuntime } from "effect";

import {
	DecodeBrowserOrigin,
	ForgeControl,
	ForgeLauncher,
	ForgeLifecycle,
	make_forge_lifecycle_layer,
} from "../../modules/cli/src/lifecycle";
import {
	make_memory_instance_store,
	ForgeInstanceError,
	ForgeInstanceStore,
	type ForgeRuntimeState,
} from "../../modules/cli/src/instance";
import { AeCommand, StopForgeInstance } from "../../modules/cli/src/entry";
import { task_scheduler_arguments } from "../../modules/cli/src/adapters";

const config = {
	data_root: "C:/artisan",
	listen_host: "127.0.0.1" as const,
	listen_port: 0,
	mode: "local" as const,
	version: 1 as const,
};

const MakeRuntime = (healthy = false, instance_missing = false) => {
	let current_health = healthy;
	let starts = 0;
	let foreground = 0;
	let stopped = 0;
	const states = new Map<string, ForgeRuntimeState>();
	const instance_store = Layer.succeed(
		ForgeInstanceStore,
		ForgeInstanceStore.of({
			Ensure: () => Effect.void,
			Load: () =>
				instance_missing
					? Effect.fail(new ForgeInstanceError({ code: "missing" }))
					: Effect.succeed(config),
			LoadSecrets: () => Effect.succeed({ auth_token: "a".repeat(43), version: 1 as const }),
			ReadState: () => Effect.succeed(states.get("instance")),
			RemoveStateIfOwned: (instance_id) =>
				Effect.sync(() => {
					if (states.get("instance")?.instance_id !== instance_id) return false;
					states.delete("instance");
					return true;
				}),
			Paths: () =>
				Effect.succeed({
					config_path: "home/config.json",
					log_path: "home/forge.log",
					readiness_path: "home/ready.json",
					secrets_path: "home/secrets.json",
					state_path: "home/state.json",
				}),
		}),
	);
	const launcher = Layer.succeed(
		ForgeLauncher,
		ForgeLauncher.of({
			StartBackground: (input) =>
				Effect.sync(() => {
					starts += 1;
					current_health = true;
					states.set("instance", {
						endpoint: "http://127.0.0.1:4848",
						instance_id: input.instance_id,
						pid: 1,
						started_at: new Date().toISOString(),
						version: 1,
					});
				}),
			StartForeground: () =>
				Effect.sync(() => {
					foreground += 1;
				}),
			TerminateVerified: () =>
				Effect.sync(() => {
					stopped += 1;
				}),
		}),
	);
	const control = Layer.succeed(
		ForgeControl,
		ForgeControl.of({
			Health: () => Effect.succeed(current_health),
			Shutdown: () =>
				Effect.sync(() => {
					current_health = false;
					states.clear();
				}),
			Pair: () => Effect.succeed("one-time-code"),
		}),
	);
	return {
		counts: () => ({ foreground, starts, stopped }),
		set_health: (next: boolean) => {
			current_health = next;
		},
		states,
		runtime: ManagedRuntime.make(
			make_forge_lifecycle_layer.pipe(
				Layer.provideMerge(launcher),
				Layer.provideMerge(control),
				Layer.provideMerge(instance_store),
			),
		),
	};
};

describe("ae lifecycle contract", () => {
	it("declares the complete stable command surface", () => {
		expect(
			AeCommand.subcommands
				.flatMap((group) => group.commands)
				.map((command) => command.name)
				.toSorted(),
		).toEqual([
			"doctor",
			"instances",
			"logs",
			"open",
			"restart",
			"setup",
			"start",
			"status",
			"stop",
			"uninstall",
			"update",
		]);
	});

	it("sets up the instance config and never exposes its generated secret", async () => {
		const runtime = ManagedRuntime.make(make_memory_instance_store());
		const store = await runtime.runPromise(ForgeInstanceStore);
		await runtime.runPromise(store.Ensure(config));
		expect(await runtime.runPromise(store.Load())).toEqual(config);
		expect((await runtime.runPromise(store.LoadSecrets())).auth_token).toHaveLength(43);
		await runtime.dispose();
	});

	it("starts only once when Forge is healthy", async () => {
		const { runtime, counts } = MakeRuntime(true);
		const store = await runtime.runPromise(ForgeInstanceStore);
		await runtime.runPromise(store.Ensure(config));
		const lifecycle = await runtime.runPromise(ForgeLifecycle);
		await runtime.runPromise(lifecycle.Start());
		await runtime.runPromise(lifecycle.Start());
		expect(counts().starts).toBe(1);
		await runtime.dispose();
	});

	it("fails start and open with a typed missing-instance error without creating one", async () => {
		const { runtime, counts } = MakeRuntime(false, true);
		const lifecycle = await runtime.runPromise(ForgeLifecycle);

		await expect(runtime.runPromise(lifecycle.Start())).rejects.toMatchObject({
			code: "missing",
		});
		await expect(runtime.runPromise(lifecycle.Open())).rejects.toMatchObject({
			code: "missing",
		});
		expect(counts().starts).toBe(0);
		await runtime.dispose();
	});

	it("waits for graceful state release before restarting with a new instance", async () => {
		const { runtime, counts, states } = MakeRuntime(false);
		const store = await runtime.runPromise(ForgeInstanceStore);
		await runtime.runPromise(store.Ensure(config));
		const lifecycle = await runtime.runPromise(ForgeLifecycle);

		await runtime.runPromise(lifecycle.Start());
		const first_instance = states.get("instance")?.instance_id;
		await runtime.runPromise(lifecycle.Restart());
		const second_instance = states.get("instance")?.instance_id;

		expect(first_instance).toBeDefined();
		expect(second_instance).toBeDefined();
		expect(first_instance).toMatch(/^forge_[0-9]+$/);
		expect(second_instance).toMatch(/^forge_[0-9]+$/);
		expect(second_instance).not.toBe(first_instance);
		expect(counts().starts).toBe(2);
		await runtime.dispose();
	});

	it("reclaims a definitely stopped stale instance before a later start", async () => {
		const { runtime, states, counts, set_health } = MakeRuntime(false);
		const store = await runtime.runPromise(ForgeInstanceStore);
		await runtime.runPromise(store.Ensure(config));
		states.set("instance", {
			endpoint: "http://127.0.0.1:4848",
			instance_id: "11111111-1111-4111-8111-111111111111",
			pid: 2_147_483_647,
			started_at: new Date().toISOString(),
			version: 1,
		});
		const lifecycle = await runtime.runPromise(ForgeLifecycle);
		await runtime.runPromise(lifecycle.Stop());
		expect(states.has("instance")).toBe(false);
		expect(counts().stopped).toBe(1);
		set_health(true);
		await runtime.runPromise(lifecycle.Start());
		expect(counts().starts).toBe(1);
		await runtime.dispose();
	});

	it("reports missing, supports foreground finalization, and starts before opening a fragment pair capability", async () => {
		const { runtime, counts } = MakeRuntime(false);
		const store = await runtime.runPromise(ForgeInstanceStore);
		await runtime.runPromise(store.Ensure(config));
		const lifecycle = await runtime.runPromise(ForgeLifecycle);
		expect(await runtime.runPromise(lifecycle.Status())).toEqual({ state: "missing" });
		expect(await runtime.runPromise(lifecycle.Open())).toBe(
			"http://127.0.0.1:4848/#pair=one-time-code",
		);
		expect(counts().starts).toBe(1);
		await runtime.runPromise(lifecycle.Stop());
		await runtime.runPromise(lifecycle.Start(true));
		expect(counts().foreground).toBe(1);
		await runtime.dispose();
	});

	it("constructs a current-user limited Windows autostart task without host mutation", () => {
		expect(
			task_scheduler_arguments({
				enabled: true,
				executable_path: "C:/Program Files/Artisan/ae.exe",
			}),
		).toEqual(expect.arrayContaining(["/SC", "ONLOGON", "/RL", "LIMITED"]));
	});

	it("stops the home's Forge instance before uninstall", async () => {
		let stopped = 0;
		const lifecycle = Layer.succeed(
			ForgeLifecycle,
			ForgeLifecycle.of({
				Doctor: () => Effect.die("not used"),
				Open: () => Effect.die("not used"),
				PairHandoff: () => Effect.die("not used"),
				Restart: () => Effect.die("not used"),
				Start: () => Effect.die("not used"),
				Status: () => Effect.die("not used"),
				Stop: () =>
					Effect.sync(() => {
						stopped += 1;
					}),
			}),
		);
		await Effect.runPromise(StopForgeInstance.pipe(Effect.provide(lifecycle)));
		expect(stopped).toBe(1);
	});

	it("accepts only explicitly local browser origins for pairing links", async () => {
		expect(
			await Effect.runPromise(DecodeBrowserOrigin("https://artisan-editor.localhost:5173")),
		).toBe("https://artisan-editor.localhost:5173");
		await expect(
			Effect.runPromise(DecodeBrowserOrigin("https://example.com")),
		).rejects.toMatchObject({
			_tag: "ForgeLifecycleError",
		});
	});
});
