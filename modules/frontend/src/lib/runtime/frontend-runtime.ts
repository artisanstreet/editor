import { BrowserKeyValueStore } from "@effect/platform-browser";
import { Layer } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

import { make_artisan_client_layer, TransportRuntimeLive } from "@artisan/transport/client";

import { FrontendMessagePortConnectorLive } from "./desktop-message-port-connector";
import { ShellPresentationPreferencesLive } from "./shell-presentation-preferences";

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

/** Production runtime composition. Fixture clients are never included here. */
export const FrontendRuntimeLive = Layer.merge(
	ShellPresentationPreferencesRuntimeLive,
	ArtisanClientRuntimeLive,
);
