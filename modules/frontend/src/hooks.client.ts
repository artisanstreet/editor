import { ClientRuntime } from "svelte-effect-runtime";

import { BrowserFrontendRuntimeLive } from "$lib/runtime/browser-frontend-runtime";

export const init = () => {
	ClientRuntime.make(BrowserFrontendRuntimeLive);
};
