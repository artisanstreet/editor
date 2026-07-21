import { BrowserKeyValueStore } from "@effect/platform-browser";
import { Effect, Exit, Layer, Scope } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

import { make_artisan_client_layer, TransportRuntimeLive } from "@artisan/transport/client";

import { FrontendMessagePortConnectorLive } from "./desktop-message-port-connector";
import { ShellPresentationPreferencesLive } from "./shell-presentation-preferences";
import { LiveWorkspaceStoreLive } from "../live-workspace/store";

export const RecoverKeyValueStore = Layer.catchCause(() => KeyValueStore.layerMemory);

const ResilientBrowserKeyValueStoreLive =
	BrowserKeyValueStore.layerLocalStorage.pipe(RecoverKeyValueStore);

const ShellPresentationPreferencesRuntimeLive = Layer.provide(
	ShellPresentationPreferencesLive,
	ResilientBrowserKeyValueStoreLive,
);

const ArtisanClientRuntimeLive = make_artisan_client_layer().pipe(
	Layer.provideMerge(FrontendMessagePortConnectorLive),
	Layer.provide(TransportRuntimeLive),
);

const LiveWorkspaceRuntimeLive = LiveWorkspaceStoreLive.pipe(
	Layer.provideMerge(ArtisanClientRuntimeLive),
);

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
	LiveWorkspaceRuntimeLive,
	FrontendComponentScopeLive,
);
