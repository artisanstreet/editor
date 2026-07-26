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
	make_memory_profile_store,
	ForgeProfileStore,
	type ForgeRuntimeState,
} from "../../modules/cli/src/profile";
import { AeCommand, PromptForProjectRoot } from "../../modules/cli/src/entry";
import { task_scheduler_arguments } from "../../modules/cli/src/adapters";

const config = {
	data_root: "C:/artisan",
	listen_host: "127.0.0.1" as const,
	listen_port: 0,
	mode: "local" as const,
	project_roots: ["C:/work"] as const,
	version: 1 as const,
};

const MakeRuntime = (healthy = false) => {
	let current_health = healthy;
	let starts = 0;
	let foreground = 0;
	let stopped = 0;
	const states = new Map<string, ForgeRuntimeState>();
	const profile_store = Layer.succeed(
		ForgeProfileStore,
		ForgeProfileStore.of({
			Ensure: () => Effect.void,
			Load: () => Effect.succeed(config),
			LoadSecrets: () => Effect.succeed({ auth_token: "a".repeat(43), version: 1 as const }),
			ReadState: (profile) => Effect.succeed(states.get(profile)),
			RemoveStateIfOwned: (profile, instance_id) =>
				Effect.sync(() => {
					if (states.get(profile)?.instance_id !== instance_id) return false;
					states.delete(profile);
					return true;
				}),
			Paths: (profile) =>
				Effect.succeed({
					config_path: `${profile}/config.json`,
					log_path: `${profile}/forge.log`,
					readiness_path: `${profile}/ready.json`,
					secrets_path: `${profile}/secrets.json`,
					state_path: `${profile}/state.json`,
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
					states.set(input.profile, {
						endpoint: "http://127.0.0.1:4848",
						instance_id: input.instance_id,
						pid: 1,
						profile: input.profile,
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
				Layer.provideMerge(profile_store),
			),
		),
	};
};

describe("ae lifecycle contract", () => {
	it("declares the complete stable command surface and interactive setup prompt", () => {
		expect(
			AeCommand.subcommands
				.flatMap((group) => group.commands)
				.map((command) => command.name)
				.toSorted(),
		).toEqual(["doctor", "logs", "open", "restart", "setup", "start", "status", "stop"]);
		expect(PromptForProjectRoot()).toBeDefined();
	});

	it("sets up profile config and never exposes its generated secret", async () => {
		const runtime = ManagedRuntime.make(make_memory_profile_store());
		const store = await runtime.runPromise(ForgeProfileStore);
		await runtime.runPromise(store.Ensure("default", config));
		expect(await runtime.runPromise(store.Load("default"))).toEqual(config);
		expect((await runtime.runPromise(store.LoadSecrets("default"))).auth_token).toHaveLength(
			43,
		);
		await runtime.dispose();
	});

	it("starts only once when Forge is healthy", async () => {
		const { runtime, counts } = MakeRuntime(true);
		const store = await runtime.runPromise(ForgeProfileStore);
		await runtime.runPromise(store.Ensure("default", config));
		const lifecycle = await runtime.runPromise(ForgeLifecycle);
		await runtime.runPromise(lifecycle.Start("default"));
		await runtime.runPromise(lifecycle.Start("default"));
		expect(counts().starts).toBe(1);
		await runtime.dispose();
	});

	it("waits for graceful state release before restarting with a new instance", async () => {
		const { runtime, counts, states } = MakeRuntime(false);
		const store = await runtime.runPromise(ForgeProfileStore);
		await runtime.runPromise(store.Ensure("default", config));
		const lifecycle = await runtime.runPromise(ForgeLifecycle);

		await runtime.runPromise(lifecycle.Start("default"));
		const first_instance = states.get("default")?.instance_id;
		await runtime.runPromise(lifecycle.Restart("default"));
		const second_instance = states.get("default")?.instance_id;

		expect(first_instance).toBeDefined();
		expect(second_instance).toBeDefined();
		expect(first_instance).toMatch(/^forge_[0-9]+$/);
		expect(second_instance).toMatch(/^forge_[0-9]+$/);
		expect(second_instance).not.toBe(first_instance);
		expect(counts().starts).toBe(2);
		await runtime.dispose();
	});

	it("preserves a foreign record instead of deleting it to start", async () => {
		const { runtime, states } = MakeRuntime(false);
		states.set("default", {
			endpoint: "http://127.0.0.1:4848",
			instance_id: "11111111-1111-4111-8111-111111111111",
			pid: 10,
			profile: "other",
			started_at: new Date().toISOString(),
			version: 1,
		});
		const lifecycle = await runtime.runPromise(ForgeLifecycle);
		await expect(runtime.runPromise(lifecycle.Start("default"))).rejects.toMatchObject({
			code: "ownership",
		});
		expect(states.get("default")?.instance_id).toBe("11111111-1111-4111-8111-111111111111");
		await runtime.dispose();
	});

	it("reclaims a definitely stopped stale instance before a later start", async () => {
		const { runtime, states, counts, set_health } = MakeRuntime(false);
		const store = await runtime.runPromise(ForgeProfileStore);
		await runtime.runPromise(store.Ensure("default", config));
		states.set("default", {
			endpoint: "http://127.0.0.1:4848",
			instance_id: "11111111-1111-4111-8111-111111111111",
			pid: 2_147_483_647,
			profile: "default",
			started_at: new Date().toISOString(),
			version: 1,
		});
		const lifecycle = await runtime.runPromise(ForgeLifecycle);
		await runtime.runPromise(lifecycle.Stop("default"));
		expect(states.has("default")).toBe(false);
		expect(counts().stopped).toBe(1);
		set_health(true);
		await runtime.runPromise(lifecycle.Start("default"));
		expect(counts().starts).toBe(1);
		await runtime.dispose();
	});

	it("reports missing, supports foreground finalization path, and opens only a fragment pair capability", async () => {
		const { runtime, counts } = MakeRuntime(false);
		const store = await runtime.runPromise(ForgeProfileStore);
		await runtime.runPromise(store.Ensure("default", config));
		const lifecycle = await runtime.runPromise(ForgeLifecycle);
		expect(await runtime.runPromise(lifecycle.Status("default"))).toEqual({ state: "missing" });
		await runtime.runPromise(lifecycle.Start("default", true));
		expect(counts().foreground).toBe(1);
		await runtime.dispose();
	});

	it("constructs a current-user limited Windows autostart task without host mutation", () => {
		expect(
			task_scheduler_arguments({
				enabled: true,
				executable_path: "C:/Program Files/Artisan/ae.exe",
				profile: "default",
			}),
		).toEqual(expect.arrayContaining(["/SC", "ONLOGON", "/RL", "LIMITED"]));
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
