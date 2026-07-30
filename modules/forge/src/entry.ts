import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

import { NodeRuntime } from "@effect/platform-node-shared";
import { MakeSnowflakeIdLive, SnowflakeId } from "@artisan/protocol";
import { Clock, Console, Effect, Layer, Schema } from "effect";

import { decode_forge_config } from "./config";
import { StartForge } from "./forge-host";
import { ResolveInstanceRegistryRoot } from "./instance-registry";
import { MaintainBoundedForgeLog } from "./log-retention";
import { RemoveForgeState, WriteForgeState } from "./state";

const ForgeParentMessage = Schema.Struct({
	kind: Schema.Literal("artisan:forge-shutdown"),
});

const RequiredEnvironment = (name: string) =>
	Effect.gen(function* () {
		const value = process.env[name];
		if (value === undefined || value.length === 0) {
			return yield* Effect.fail(new Error(`Artisan Forge requires ${name}`));
		}
		return value;
	});

const AwaitProcessShutdown = Effect.callback<void>((resume) => {
	const shutdown = () => resume(Effect.void);
	const message = (value: unknown) => {
		if (Schema.is(ForgeParentMessage)(value)) shutdown();
	};

	process.once("SIGINT", shutdown);
	process.once("SIGTERM", shutdown);
	if (process.connected) process.once("disconnect", shutdown);
	process.on("message", message);

	return Effect.sync(() => {
		process.removeListener("SIGINT", shutdown);
		process.removeListener("SIGTERM", shutdown);
		process.removeListener("disconnect", shutdown);
		process.removeListener("message", message);
	});
});

/** Runs the headless Forge lifecycle inside the executable's single Effect runtime. */
export const StartForgeFromEnvironment = Effect.gen(function* () {
	const snowflake_id = yield* SnowflakeId;
	const instance_id =
		process.env.ARTISAN_FORGE_INSTANCE_ID ?? (yield* snowflake_id.Make("forge"));
	const database_path = yield* RequiredEnvironment("ARTISAN_DATABASE_PATH");
	const migrations_path = yield* RequiredEnvironment("ARTISAN_MIGRATIONS_PATH");
	const instance_registry_root =
		process.env.ARTISAN_INSTANCE_REGISTRY_ROOT ?? ResolveInstanceRegistryRoot(process.env);
	const host = yield* StartForge(
		decode_forge_config({
			database_path,
			instance_id,
			...(instance_registry_root === undefined ? {} : { instance_registry_root }),
			listen_host: process.env.ARTISAN_LISTEN_HOST === "::1" ? "::1" : "127.0.0.1",
			listen_port: process.env.ARTISAN_LISTEN_PORT
				? Number(process.env.ARTISAN_LISTEN_PORT)
				: 0,
			migrations_path,
			...(process.env.ARTISAN_ALLOWED_ORIGINS === undefined
				? {}
				: {
						allowed_origins: process.env.ARTISAN_ALLOWED_ORIGINS.split(",")
							.map((origin) => origin.trim())
							.filter(Boolean),
					}),
			...(process.env.ARTISAN_AUTH_TOKEN === undefined
				? {}
				: { auth_token: process.env.ARTISAN_AUTH_TOKEN }),
			...(process.env.ARTISAN_FORGE_DEVELOPMENT === "1" ? { development: true } : {}),
			...(process.env.ARTISAN_STATIC_FRONTEND_ROOT === undefined
				? {}
				: {
						static_frontend_root: process.env.ARTISAN_STATIC_FRONTEND_ROOT,
					}),
		}),
	);
	const state_path = process.env.ARTISAN_FORGE_STATE_PATH;
	const log_path = process.env.ARTISAN_FORGE_LOG_PATH;
	if (log_path !== undefined) {
		yield* Effect.forkScoped(MaintainBoundedForgeLog(log_path));
	}

	yield* Effect.acquireUseRelease(
		Effect.gen(function* () {
			if (state_path !== undefined) {
				const now = yield* Clock.currentTimeMillis;
				yield* WriteForgeState(state_path, {
					endpoint: host.endpoint.toString(),
					instance_id,
					pid: process.pid,
					started_at: new Date(now).toISOString(),
					version: 1,
				});
			}

			yield* Console.log(
				JSON.stringify({
					endpoint: host.endpoint.toString(),
					kind: "artisan:forge-ready",
					pid: process.pid,
				}),
			);

			return host;
		}),
		(current_host) => Effect.raceFirst(current_host.ShutdownRequested, AwaitProcessShutdown),
		(current_host) =>
			Effect.gen(function* () {
				yield* current_host.Close;
				if (state_path !== undefined) {
					yield* RemoveForgeState(state_path, instance_id);
				}
				process.exitCode = 0;
			}),
	);
}).pipe(
	Effect.scoped,
	Effect.ensuring(
		Effect.sync(() => {
			if (process.connected) process.disconnect?.();
		}),
	),
);

if (process.argv[1] !== undefined && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
	NodeRuntime.runMain(
		StartForgeFromEnvironment.pipe(Effect.provide(MakeSnowflakeIdLive(3).pipe(Layer.orDie))),
	);
}
