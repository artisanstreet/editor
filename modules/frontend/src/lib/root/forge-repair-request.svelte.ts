import { Effect } from "effect";

import { RunBrowserDom } from "../browser/dom";

import { AttentionMarkedTitle } from "./attention-title";

/**
 * Renderer-initiated ask for the desktop shell to re-run its Forge handoff.
 *
 * The renderer has no IPC, so the ask travels over the document-title channel
 * the shell already observes for connection loss. This module is the shared
 * switch: surfaces that want the shell's own Forge back (the Machine select
 * returning from a peer) flip it, and the attention-title marker keeps
 * republishing the repair marker for as long as it stays set — one persistent
 * writer instead of two racing over the title string.
 */
export const forge_repair_request = $state({ requested: false });

export const RequestForgeRepair = Effect.gen(function* () {
	forge_repair_request.requested = true;
	yield* RunBrowserDom(() => {
		const marked = AttentionMarkedTitle(document.title, undefined, true);
		if (document.title !== marked) document.title = marked;
	}).pipe(Effect.ignore);
});
