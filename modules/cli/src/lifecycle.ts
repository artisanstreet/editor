import { MakeSnowflakeIdLive, SnowflakeId } from "@artisan/protocol";
import { Context, Data, Effect, Layer, Semaphore } from "effect";

import {
	ForgeInstanceError,
	ForgeInstanceStore,
	type ForgeInstanceConfig,
	type ForgeRuntimeState,
} from "./instance";

export class ForgeLifecycleError extends Data.TaggedError("ForgeLifecycleError")<{
	readonly cause?: unknown;
	readonly code: "not_running" | "ownership" | "timeout" | "unsupported";
}> {}

export class ForgeLauncher extends Context.Service<
	ForgeLauncher,
	{
		readonly StartBackground: (input: {
			readonly config: ForgeInstanceConfig;
			readonly instance_id: string;
			readonly token: string;
		}) => Effect.Effect<void, ForgeLifecycleError>;
		readonly StartForeground: (input: {
			readonly config: ForgeInstanceConfig;
			readonly instance_id: string;
			readonly token: string;
		}) => Effect.Effect<void, ForgeLifecycleError>;
		readonly TerminateVerified: (
			state: ForgeRuntimeState,
		) => Effect.Effect<void, ForgeLifecycleError>;
	}
>()("Artisan/ForgeLauncher") {}

export class ForgeControl extends Context.Service<
	ForgeControl,
	{
		readonly Health: (
			endpoint: string,
			instance_id: string,
			token: string,
		) => Effect.Effect<boolean, ForgeLifecycleError>;
		readonly Shutdown: (
			endpoint: string,
			token: string,
		) => Effect.Effect<void, ForgeLifecycleError>;
		readonly Pair: (
			endpoint: string,
			token: string,
		) => Effect.Effect<string, ForgeLifecycleError>;
	}
>()("Artisan/ForgeControl") {}

type ForgeStatus = "missing" | "running" | "stale";

export class ForgeLifecycle extends Context.Service<
	ForgeLifecycle,
	{
		readonly Doctor: () => Effect.Effect<
			{ readonly healthy: boolean; readonly state: ForgeStatus },
			ForgeLifecycleError | ForgeInstanceError
		>;
		readonly Open: (
			origin?: string,
		) => Effect.Effect<string, ForgeLifecycleError | ForgeInstanceError>;
		/**
		 * Starts the home's Forge when needed and mints one one-time pairing
		 * capability for a trusted local caller such as the installed editor.
		 * The code is single-use and short-lived; nothing durable is returned.
		 */
		readonly PairHandoff: () => Effect.Effect<
			{ readonly endpoint: string; readonly pair_code: string },
			ForgeLifecycleError | ForgeInstanceError
		>;
		readonly Restart: (
			foreground?: boolean,
		) => Effect.Effect<void, ForgeLifecycleError | ForgeInstanceError>;
		readonly Start: (
			foreground?: boolean,
		) => Effect.Effect<void, ForgeLifecycleError | ForgeInstanceError>;
		readonly Status: () => Effect.Effect<
			{ readonly state: ForgeStatus },
			ForgeLifecycleError | ForgeInstanceError
		>;
		readonly Stop: () => Effect.Effect<void, ForgeLifecycleError | ForgeInstanceError>;
	}
>()("Artisan/ForgeLifecycle") {}

/** Owns lifecycle ordering; adapters own OS process, HTTP, and browser details. */
export const make_forge_lifecycle_layer = Layer.effect(
	ForgeLifecycle,
	Effect.gen(function* () {
		const store = yield* ForgeInstanceStore;
		const launcher = yield* ForgeLauncher;
		const control = yield* ForgeControl;
		const snowflake_id = yield* SnowflakeId;
		const lifecycle_mutex = yield* Semaphore.make(1);

		const Status = () =>
			Effect.gen(function* () {
				const state = yield* store
					.ReadState()
					.pipe(Effect.catch(() => Effect.succeed(undefined)));
				if (!state) return { state: "missing" as const };
				const secret = yield* store.LoadSecrets();
				const healthy = yield* control
					.Health(state.endpoint, state.instance_id, secret.auth_token)
					.pipe(Effect.catch(() => Effect.succeed(false)));
				return { state: healthy ? ("running" as const) : ("stale" as const) };
			});

		const WaitForReady = (
			instance_id: string,
			token: string,
		): Effect.Effect<void, ForgeLifecycleError> =>
			Effect.gen(function* () {
				const state = yield* store
					.ReadState()
					.pipe(Effect.catch(() => Effect.succeed(undefined)));
				if (state?.instance_id === instance_id) {
					const healthy = yield* control
						.Health(state.endpoint, instance_id, token)
						.pipe(Effect.catch(() => Effect.succeed(false)));
					if (healthy) return;
				}
				yield* Effect.sleep("100 millis");
				yield* WaitForReady(instance_id, token);
			}).pipe(
				Effect.timeout("15 seconds"),
				Effect.mapError((cause) => new ForgeLifecycleError({ cause, code: "timeout" })),
			);

		const WaitForStopped = (instance_id: string): Effect.Effect<void, ForgeLifecycleError> =>
			Effect.gen(function* () {
				const state = yield* store
					.ReadState()
					.pipe(Effect.catch(() => Effect.succeed(undefined)));
				if (state?.instance_id !== instance_id) return;
				yield* Effect.sleep("100 millis");
				yield* WaitForStopped(instance_id);
			}).pipe(
				Effect.timeout("15 seconds"),
				Effect.mapError((cause) => new ForgeLifecycleError({ cause, code: "timeout" })),
			);

		const Start = (foreground = false) =>
			lifecycle_mutex.withPermits(1)(
				Effect.gen(function* () {
					const status = yield* Status();
					if (status.state === "running") return;
					if (status.state === "stale") {
						const stale = yield* store.ReadState();
						if (!stale)
							return yield* Effect.fail(
								new ForgeLifecycleError({ code: "ownership" }),
							);
						yield* launcher.TerminateVerified(stale);
						yield* store.RemoveStateIfOwned(stale.instance_id);
					}
					const config = yield* store.Load();
					const secret = yield* store.LoadSecrets();
					const instance_id = yield* snowflake_id.Make("forge");
					const input = { config, instance_id, token: secret.auth_token };
					if (foreground) return yield* launcher.StartForeground(input);
					yield* launcher.StartBackground(input);
					yield* WaitForReady(instance_id, secret.auth_token).pipe(
						Effect.onError(() =>
							store.RemoveStateIfOwned(instance_id).pipe(Effect.ignore),
						),
					);
				}),
			);

		const Stop = () =>
			lifecycle_mutex.withPermits(1)(
				Effect.gen(function* () {
					const state = yield* store.ReadState();
					if (!state) return;
					const secret = yield* store.LoadSecrets();
					const healthy = yield* control
						.Health(state.endpoint, state.instance_id, secret.auth_token)
						.pipe(Effect.catch(() => Effect.succeed(false)));
					if (healthy) {
						yield* control.Shutdown(state.endpoint, secret.auth_token);
						yield* WaitForStopped(state.instance_id);
						return;
					}
					yield* launcher.TerminateVerified(state);
					yield* store.RemoveStateIfOwned(state.instance_id);
				}),
			);

		const PairHandoff = () =>
			Effect.gen(function* () {
				yield* Start();
				const state = yield* store.ReadState();
				if (!state)
					return yield* Effect.fail(new ForgeLifecycleError({ code: "not_running" }));
				const secret = yield* store.LoadSecrets();
				const healthy = yield* control
					.Health(state.endpoint, state.instance_id, secret.auth_token)
					.pipe(Effect.catch(() => Effect.succeed(false)));
				if (!healthy)
					return yield* Effect.fail(new ForgeLifecycleError({ code: "not_running" }));
				const pair_code = yield* control.Pair(state.endpoint, secret.auth_token);
				return { endpoint: state.endpoint, pair_code };
			});

		return ForgeLifecycle.of({
			Doctor: () =>
				Status().pipe(
					Effect.map((result) => ({ healthy: result.state === "running", ...result })),
				),
			Open: (origin) =>
				Effect.gen(function* () {
					const handoff = yield* PairHandoff();
					const browser_origin =
						origin === undefined
							? handoff.endpoint
							: yield* DecodeBrowserOrigin(origin);
					return `${browser_origin.replace(/\/$/, "")}/#pair=${encodeURIComponent(handoff.pair_code)}`;
				}),
			PairHandoff,
			Restart: (foreground) => Effect.andThen(Stop(), Start(foreground)),
			Start,
			Status,
			Stop,
		});
	}),
).pipe(Layer.provide(MakeSnowflakeIdLive(2).pipe(Layer.orDie)));

const allowed_browser_hosts = new Set([
	"127.0.0.1",
	"[::1]",
	"localhost",
	"artisan-editor.localhost",
]);

/** The browser may use a forwarded local origin; Forge itself remains loopback-only. */
export const DecodeBrowserOrigin = (
	candidate: string,
): Effect.Effect<string, ForgeLifecycleError> =>
	Effect.try({
		catch: () => new ForgeLifecycleError({ code: "unsupported" }),
		try: () => {
			const url = new URL(candidate);
			if (
				(url.protocol !== "http:" && url.protocol !== "https:") ||
				!allowed_browser_hosts.has(url.hostname === "::1" ? "[::1]" : url.hostname) ||
				url.pathname !== "/" ||
				url.search !== "" ||
				url.hash !== ""
			)
				throw new Error("Unsupported browser origin");
			return url.origin;
		},
	});
