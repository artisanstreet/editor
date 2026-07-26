import { MakeSnowflakeIdLive, SnowflakeId } from "@artisan/protocol";
import { Context, Data, Effect, Layer, Semaphore } from "effect";

import {
	ForgeProfileError,
	ForgeProfileStore,
	type ForgeProfileConfig,
	type ForgeRuntimeState,
} from "./profile";

export class ForgeLifecycleError extends Data.TaggedError("ForgeLifecycleError")<{
	readonly cause?: unknown;
	readonly code: "not_running" | "ownership" | "timeout" | "unsupported";
}> {}

export class ForgeLauncher extends Context.Service<
	ForgeLauncher,
	{
		readonly StartBackground: (input: {
			readonly config: ForgeProfileConfig;
			readonly instance_id: string;
			readonly profile: string;
			readonly token: string;
		}) => Effect.Effect<void, ForgeLifecycleError>;
		readonly StartForeground: (input: {
			readonly config: ForgeProfileConfig;
			readonly instance_id: string;
			readonly profile: string;
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

type ForgeStatus = "missing" | "running" | "stale" | "foreign";

export class ForgeLifecycle extends Context.Service<
	ForgeLifecycle,
	{
		readonly Doctor: (
			profile: string,
		) => Effect.Effect<
			{ readonly healthy: boolean; readonly state: ForgeStatus },
			ForgeLifecycleError | ForgeProfileError
		>;
		readonly Open: (
			profile: string,
			origin?: string,
		) => Effect.Effect<string, ForgeLifecycleError | ForgeProfileError>;
		readonly Restart: (
			profile: string,
			foreground?: boolean,
		) => Effect.Effect<void, ForgeLifecycleError | ForgeProfileError>;
		readonly Start: (
			profile: string,
			foreground?: boolean,
		) => Effect.Effect<void, ForgeLifecycleError | ForgeProfileError>;
		readonly Status: (
			profile: string,
		) => Effect.Effect<
			{ readonly state: ForgeStatus },
			ForgeLifecycleError | ForgeProfileError
		>;
		readonly Stop: (
			profile: string,
		) => Effect.Effect<void, ForgeLifecycleError | ForgeProfileError>;
	}
>()("Artisan/ForgeLifecycle") {}

/** Owns lifecycle ordering; adapters own OS process, HTTP, and browser details. */
export const make_forge_lifecycle_layer = Layer.effect(
	ForgeLifecycle,
	Effect.gen(function* () {
		const store = yield* ForgeProfileStore;
		const launcher = yield* ForgeLauncher;
		const control = yield* ForgeControl;
		const snowflake_id = yield* SnowflakeId;
		const lifecycle_mutex = yield* Semaphore.make(1);

		const Status = (profile: string) =>
			Effect.gen(function* () {
				const state = yield* store
					.ReadState(profile)
					.pipe(Effect.catch(() => Effect.succeed(undefined)));
				if (!state) return { state: "missing" as const };
				if (state.profile !== profile) return { state: "foreign" as const };
				const secret = yield* store.LoadSecrets(profile);
				const healthy = yield* control
					.Health(state.endpoint, state.instance_id, secret.auth_token)
					.pipe(Effect.catch(() => Effect.succeed(false)));
				return { state: healthy ? ("running" as const) : ("stale" as const) };
			});

		const WaitForReady = (
			profile: string,
			instance_id: string,
			token: string,
		): Effect.Effect<void, ForgeLifecycleError> =>
			Effect.gen(function* () {
				const state = yield* store
					.ReadState(profile)
					.pipe(Effect.catch(() => Effect.succeed(undefined)));
				if (state?.instance_id === instance_id && state.profile === profile) {
					const healthy = yield* control
						.Health(state.endpoint, instance_id, token)
						.pipe(Effect.catch(() => Effect.succeed(false)));
					if (healthy) return;
				}
				yield* Effect.sleep("100 millis");
				yield* WaitForReady(profile, instance_id, token);
			}).pipe(
				Effect.timeout("15 seconds"),
				Effect.mapError((cause) => new ForgeLifecycleError({ cause, code: "timeout" })),
			);

		const WaitForStopped = (
			profile: string,
			instance_id: string,
		): Effect.Effect<void, ForgeLifecycleError> =>
			Effect.gen(function* () {
				const state = yield* store
					.ReadState(profile)
					.pipe(Effect.catch(() => Effect.succeed(undefined)));
				if (state?.instance_id !== instance_id) return;
				yield* Effect.sleep("100 millis");
				yield* WaitForStopped(profile, instance_id);
			}).pipe(
				Effect.timeout("15 seconds"),
				Effect.mapError((cause) => new ForgeLifecycleError({ cause, code: "timeout" })),
			);

		const Start = (profile: string, foreground = false) =>
			lifecycle_mutex.withPermits(1)(
				Effect.gen(function* () {
					const status = yield* Status(profile);
					if (status.state === "running") return;
					if (status.state === "foreign")
						return yield* Effect.fail(new ForgeLifecycleError({ code: "ownership" }));
					if (status.state === "stale") {
						const stale = yield* store.ReadState(profile);
						if (!stale || stale.profile !== profile)
							return yield* Effect.fail(
								new ForgeLifecycleError({ code: "ownership" }),
							);
						yield* launcher.TerminateVerified(stale);
						yield* store.RemoveStateIfOwned(profile, stale.instance_id);
					}
					const config = yield* store.Load(profile);
					const secret = yield* store.LoadSecrets(profile);
					const instance_id = yield* snowflake_id.Make("forge");
					const input = { config, instance_id, profile, token: secret.auth_token };
					if (foreground) return yield* launcher.StartForeground(input);
					yield* launcher.StartBackground(input);
					yield* WaitForReady(profile, instance_id, secret.auth_token).pipe(
						Effect.onError(() =>
							store.RemoveStateIfOwned(profile, instance_id).pipe(Effect.ignore),
						),
					);
				}),
			);

		const Stop = (profile: string) =>
			lifecycle_mutex.withPermits(1)(
				Effect.gen(function* () {
					const state = yield* store.ReadState(profile);
					if (!state) return;
					if (state.profile !== profile)
						return yield* Effect.fail(new ForgeLifecycleError({ code: "ownership" }));
					const secret = yield* store.LoadSecrets(profile);
					const healthy = yield* control
						.Health(state.endpoint, state.instance_id, secret.auth_token)
						.pipe(Effect.catch(() => Effect.succeed(false)));
					if (healthy) {
						yield* control.Shutdown(state.endpoint, secret.auth_token);
						yield* WaitForStopped(profile, state.instance_id);
						return;
					}
					yield* launcher.TerminateVerified(state);
					yield* store.RemoveStateIfOwned(profile, state.instance_id);
				}),
			);

		return ForgeLifecycle.of({
			Doctor: (profile) =>
				Status(profile).pipe(
					Effect.map((result) => ({ healthy: result.state === "running", ...result })),
				),
			Open: (profile, origin) =>
				Effect.gen(function* () {
					const state = yield* store.ReadState(profile);
					if (!state || state.profile !== profile)
						return yield* Effect.fail(new ForgeLifecycleError({ code: "not_running" }));
					const secret = yield* store.LoadSecrets(profile);
					const healthy = yield* control
						.Health(state.endpoint, state.instance_id, secret.auth_token)
						.pipe(Effect.catch(() => Effect.succeed(false)));
					if (!healthy)
						return yield* Effect.fail(new ForgeLifecycleError({ code: "not_running" }));
					const code = yield* control.Pair(state.endpoint, secret.auth_token);
					const browser_origin =
						origin === undefined ? state.endpoint : yield* DecodeBrowserOrigin(origin);
					return `${browser_origin.replace(/\/$/, "")}/#pair=${encodeURIComponent(code)}`;
				}),
			Restart: (profile, foreground) =>
				Effect.andThen(Stop(profile), Start(profile, foreground)),
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
