import { BrowserHttpClient } from "@effect/platform-browser";
import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { Effect, Exit, Layer, Scope } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

import { AppearancePreferencesLive } from "./appearance-preferences";
import { ForgeEndpointStoreLive, ResolveForgeEndpoint } from "./forge-endpoint";
import {
	BootstrapBrowserPairing,
	MakeBrowserNavigationLive,
	BrowserPairingExchangeLive,
} from "./pairing";
import { ShellPresentationPreferencesLive } from "./shell-presentation-preferences";
import {
	make_websocket_client_runtime_layer,
	ResolveWebSocketRuntimeTarget,
} from "./websocket-runtime";

export const RecoverKeyValueStore = Layer.catchCause(() => KeyValueStore.layerMemory);

const ShellPresentationPreferencesRuntimeLive = Layer.provide(
	ShellPresentationPreferencesLive,
	KeyValueStore.layerMemory,
);

/**
 * Backed by real storage rather than memory: a preference a reader sets in
 * settings has to survive the reload that follows it, and a browser that
 * refuses storage falls back to memory instead of failing the runtime.
 */
const AppearancePreferencesRuntimeLive = Layer.provide(
	AppearancePreferencesLive,
	KeyValueStore.layerStorage(() => localStorage).pipe(RecoverKeyValueStore),
);

const ArtisanClientRuntimeLive = Layer.unwrap(
	Effect.gen(function* () {
		const renderer_window = (
			globalThis as {
				readonly window?: {
					readonly history?: {
						readonly replaceState: (data: null, unused: string, url: string) => void;
					};
					readonly location?: {
						readonly hash: string;
						readonly origin: string;
						readonly pathname: string;
						readonly protocol: string;
						readonly search: string;
					};
				};
			}
		).window;
		if (renderer_window?.location !== undefined && renderer_window.history !== undefined) {
			yield* BootstrapBrowserPairing.pipe(
				Effect.provide(
					Layer.merge(
						MakeBrowserNavigationLive({
							history: renderer_window.history,
							location: renderer_window.location,
						}),
						Layer.merge(
							ForgeEndpointStoreLive,
							BrowserPairingExchangeLive.pipe(
								/**
								 * The app-scheme renderer pairs cross-origin against the
								 * adopted loopback Forge, so its session cookie only
								 * exists when the exchange carries credentials. On a
								 * same-origin page this is identical to the default.
								 */
								Layer.provide(
									BrowserHttpClient.layerFetch.pipe(
										Layer.provide(
											Layer.succeed(BrowserHttpClient.RequestInit, {
												credentials: "include",
											}),
										),
									),
								),
							),
						),
					),
				),
			);
		}
		const environment = (
			import.meta as ImportMeta & {
				readonly env?: {
					readonly DEV?: boolean;
					readonly VITE_ARTISAN_FORGE_WS_URL?: unknown;
				};
			}
		).env;
		const forge_endpoint = yield* ResolveForgeEndpoint.pipe(
			Effect.provide(ForgeEndpointStoreLive),
		);
		const target = ResolveWebSocketRuntimeTarget({
			...(environment?.VITE_ARTISAN_FORGE_WS_URL === undefined
				? {}
				: { development_url: environment.VITE_ARTISAN_FORGE_WS_URL }),
			...(forge_endpoint === undefined ? {} : { forge_endpoint }),
			is_development: environment?.DEV === true,
			...(renderer_window?.location === undefined
				? {}
				: { location: renderer_window.location }),
		});

		return make_websocket_client_runtime_layer(target);
	}),
);

const SnowflakeIdRuntimeLive = MakeSnowflakeIdLive(3).pipe(Layer.orDie);

/**
 * SER executes component programs through a ManagedRuntime after Layer construction.
 * Expose one app-lifetime child scope so component-owned finalizers and scoped
 * subscriptions have a concrete lifecycle that closes with that runtime.
 */
export const FrontendComponentScopeLive = Layer.effect(
	Scope.Scope,
	Effect.gen(function* () {
		const AcquireComponentScope = Effect.gen(function* () {
			return yield* Scope.make();
		});
		return yield* Effect.acquireRelease(AcquireComponentScope, (scope) =>
			Effect.gen(function* () {
				yield* Scope.close(scope, Exit.void);
			}),
		);
	}),
);

/** Production runtime composition. Fixture clients are never included here. */
export const FrontendRuntimeLive = Layer.mergeAll(
	AppearancePreferencesRuntimeLive,
	ShellPresentationPreferencesRuntimeLive,
	ArtisanClientRuntimeLive,
	SnowflakeIdRuntimeLive,
	FrontendComponentScopeLive,
	ForgeEndpointStoreLive,
);
