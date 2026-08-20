import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Cause, Deferred, Effect, Exit, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { CodexModelBehaviourProbe } from "../../modules/backend/src/model-behaviour/codex-probe";
import { make_model_behaviour_config_files_layer } from "../../modules/backend/src/model-behaviour/config-files";
import {
	make_desktop_model_behaviour_provider_registry_layer,
	ModelBehaviourProviderRegistry,
} from "../../modules/backend/src/model-behaviour/provider";

const roots: Array<string> = [];

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("Model Behaviour startup loading", () => {
	it("constructs immediately and settles an external discovery waiter when its scope closes", async () => {
		const root = await fs.mkdtemp(`${tmpdir()}/artisan model behaviour startup `);
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const interrupted = await Effect.runPromise(Deferred.make<void>());
		roots.push(root);
		const probe = Layer.succeed(CodexModelBehaviourProbe, {
			Probe: Deferred.succeed(entered, undefined).pipe(
				Effect.andThen(Deferred.await(release)),
				Effect.as({
					installed_version: "0.142.5",
					mapping_available: true,
					type: "available" as const,
				}),
				Effect.onInterrupt(() => Deferred.succeed(interrupted, undefined)),
			),
		});
		const layer = make_desktop_model_behaviour_provider_registry_layer({
			backups_directory: join(root, "backups"),
			codex_config_path: join(root, "codex", "config.toml"),
		}).pipe(Layer.provide(Layer.merge(probe, make_model_behaviour_config_files_layer())));
		const runtime = ManagedRuntime.make(layer);
		const registry = await runtime.runPromise(Effect.service(ModelBehaviourProviderRegistry));

		await Effect.runPromise(Deferred.await(entered));
		const waiter = Effect.runPromiseExit(registry.Await);
		await runtime.dispose();
		const exit = await waiter;

		expect(Exit.isFailure(exit)).toBe(true);
		expect(Exit.isFailure(exit) && Cause.hasInterruptsOnly(exit.cause)).toBe(true);
		await Effect.runPromise(Deferred.await(interrupted));
		await Effect.runPromise(Deferred.succeed(release, undefined));
		const replay = await Effect.runPromiseExit(registry.Await);
		expect(Exit.isFailure(replay) && Cause.hasInterruptsOnly(replay.cause)).toBe(true);
	});
});
