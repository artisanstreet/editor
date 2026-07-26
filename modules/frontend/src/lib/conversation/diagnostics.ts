import { writable } from "svelte/store";

/** Controls whether provider and process diagnostics appear in conversation work traces. */
export const conversation_diagnostics_enabled = writable(false);
