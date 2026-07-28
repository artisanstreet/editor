/**
 * Only development Forges serve the web frontend, and `/health` reports that
 * capability as `development: true`. That single unauthenticated fact drives
 * every piece of development-instance visibility: the shell badge and the
 * document-title marker. Installed releases report `development: false` and
 * stay unmarked.
 */
export const dev_title_marker = "[Dev]";

/** True when a `/health` body identifies a development Forge. */
export const IsDevelopmentInstance = (health: unknown): boolean =>
	typeof health === "object" &&
	health !== null &&
	"development" in health &&
	(health as { readonly development?: unknown }).development === true;

/** Idempotently prefixes a document title so route-owned titles keep the marker. */
export const DevMarkedTitle = (title: string): string =>
	title.startsWith(dev_title_marker) ? title : `${dev_title_marker} ${title}`.trimEnd();
