import { Fiber } from "effect";
import { andThen, runFork } from "effect/Effect";

import type { MonacoEditorMount } from "../components/editor/monaco-editor.svelte";
import type { MonacoEditorService } from "./monaco-editor-service";

/** Bridges Svelte's synchronous mount lifecycle to the already-provided scoped service. */
export const MakeMonacoEditorMount = (
	service: typeof MonacoEditorService.Service,
): MonacoEditorMount => ({
	attach: (host) => {
		const attach_fiber = runFork(service.Attach(host));

		return () => {
			runFork(Fiber.interrupt(attach_fiber).pipe(andThen(service.Detach)));
		};
	},
});
