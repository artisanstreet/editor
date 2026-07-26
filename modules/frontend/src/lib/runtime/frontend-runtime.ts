import { BrowserHttpClient, BrowserKeyValueStore } from "@effect/platform-browser";
import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { Effect, Exit, Layer, Scope } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

import { make_artisan_client_layer, TransportRuntimeLive } from "@artisan/transport/client";

import { FrontendMessagePortConnectorLive } from "./desktop-message-port-connector";
import {
	BootstrapBrowserPairing,
	BrowserNavigationLive,
	BrowserPairingExchangeLive,
} from "./pairing";
import { ShellPresentationPreferencesLive } from "./shell-presentation-preferences";
import {
	make_websocket_client_runtime_layer,
	ResolveWebSocketRuntimeTarget,
} from "./websocket-runtime";

export const RecoverKeyValueStore = Layer.catchCause(() => KeyValueStore.layerMemory);

const ResilientBrowserKeyValueStoreLive =
	BrowserKeyValueStore.layerLocalStorage.pipe(RecoverKeyValueStore);

const ShellPresentationPreferencesRuntimeLive = Layer.provide(
	ShellPresentationPreferencesLive,
	ResilientBrowserKeyValueStoreLive,
);

const DesktopClientRuntimeLive = make_artisan_client_layer().pipe(
	Layer.provideMerge(FrontendMessagePortConnectorLive),
	Layer.provide(TransportRuntimeLive),
);

const ArtisanClientRuntimeLive = Layer.unwrap(
	Effect.gen(function* () {
		const renderer_window = (
			globalThis as {
				readonly window?: {
					readonly artisanDesktop?: {
						readonly forgeWebSocketEndpoint?: unknown;
						readonly forgeWebSocketUrl?: unknown;
						readonly websocketUrl?: unknown;
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
		if (renderer_window?.location !== undefined) {
			yield* BootstrapBrowserPairing.pipe(
				Effect.provide(
					Layer.merge(
						BrowserNavigationLive,
						BrowserPairingExchangeLive.pipe(
							Layer.provide(BrowserHttpClient.layerFetch),
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
		const target = ResolveWebSocketRuntimeTarget({
			...(renderer_window?.artisanDesktop === undefined
				? {}
				: { desktop: renderer_window.artisanDesktop }),
			...(environment?.VITE_ARTISAN_FORGE_WS_URL === undefined
				? {}
				: { development_url: environment.VITE_ARTISAN_FORGE_WS_URL }),
			is_development: environment?.DEV === true,
			...(renderer_window?.location === undefined
				? {}
				: { location: renderer_window.location }),
		});

		return target._tag === "websocket"
			? make_websocket_client_runtime_layer(target.url)
			: DesktopClientRuntimeLive;
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
	Effect.acquireRelease(Scope.make(), (scope) => Scope.close(scope, Exit.void)),
);

/** Production runtime composition. Fixture clients are never included here. */
export const FrontendRuntimeLive = Layer.mergeAll(
	ShellPresentationPreferencesRuntimeLive,
	ArtisanClientRuntimeLive,
	SnowflakeIdRuntimeLive,
	FrontendComponentScopeLive,
);
