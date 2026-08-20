/** Which shell is hosting the renderer. */
export type RuntimeSurface = "desktop" | "browser";

/**
 * The bundled desktop shell renders the same page a paired browser tab does;
 * only its user agent tells them apart. Nothing about the transport changes
 * between them — what changes is what the host already grants (notification
 * permission), what the window owns (its own frame), and therefore the copy
 * that explains either.
 */
export const RuntimeSurfaceFor = (user_agent: string): RuntimeSurface =>
	user_agent.includes("Electron/") ? "desktop" : "browser";

/**
 * macOS is the one desktop platform whose window controls overlay the frame's
 * left end rather than its right.
 */
export const IsMacDesktop = (user_agent: string): boolean =>
	RuntimeSurfaceFor(user_agent) === "desktop" && user_agent.includes("Macintosh");
