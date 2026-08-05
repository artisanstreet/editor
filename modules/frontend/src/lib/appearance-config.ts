import { writable } from "svelte/store";

/**
 * Whether glass surfaces light themselves with the shader.
 *
 * A store rather than a service because every glass surface is an ordinary
 * component that reads it while rendering; the durable value behind it lives in
 * `AppearancePreferences`, which the shell loads once at startup and the
 * settings screen writes. Starts enabled so the first frame matches the design
 * even before the stored preference has been read.
 */
export const shader_enabled = writable(true);
