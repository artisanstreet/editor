import { ClientRuntime } from "svelte-effect-runtime";

import { FrontendRuntimeLive } from "$lib/runtime/frontend-runtime";

export const init = () => {
	ClientRuntime.make(FrontendRuntimeLive);
};
