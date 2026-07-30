import { Fiber } from "effect";
import { andThen, runFork } from "effect/Effect";

import type { EditorSurfaceMount } from "../components/editor/surface.sv";
import type { EditorService } from "./service";

/** Bridges Svelte's synchronous mount lifecycle to the already-provided scoped service. */
export const MakeEditorSurfaceMount = (
	service: typeof EditorService.Service,
	options?: { readonly on_change?: () => void },
): EditorSurfaceMount => ({
	attach: (host) => {
		const attach_fiber = runFork(service.Attach(host, options));

		return () => {
			runFork(Fiber.interrupt(attach_fiber).pipe(andThen(service.Detach)));
		};
	},
});
