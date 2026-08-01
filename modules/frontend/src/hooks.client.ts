import { BrowserHttpClient } from "@effect/platform-browser";
import { Layer } from "effect";
import { ClientRuntime } from "svelte-effect-runtime";

import { BrowserFrontendRuntimeLive } from "$lib/runtime/browser-frontend-runtime";

export const init = () => {
	ClientRuntime.make(Layer.merge(BrowserFrontendRuntimeLive, BrowserHttpClient.layerFetch));
};
