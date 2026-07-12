import { BrowserKeyValueStore } from "@effect/platform-browser";
import { Layer } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";

import { ShellPresentationPreferencesLive } from "./shell-presentation-preferences";

export const RecoverKeyValueStore = Layer.catchCause(() => KeyValueStore.layerMemory);

const ResilientBrowserKeyValueStoreLive =
	BrowserKeyValueStore.layerLocalStorage.pipe(RecoverKeyValueStore);

export const FrontendRuntimeLive = Layer.provide(
	ShellPresentationPreferencesLive,
	ResilientBrowserKeyValueStoreLive,
);
