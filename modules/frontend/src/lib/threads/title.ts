import { writable } from "svelte/store";

import { DefaultThreadTitleMode, type ThreadTitleMode } from "@artisan/protocol";

/**
 * The reader's title-mode preference, as a store for the same reason the
 * appearance stores are: every surface that names a thread — rail rows, the
 * hover card, the command menu — is an ordinary component that reads it while
 * rendering. The durable value lives in `SessionDefaults`; the shell feeds
 * this store from the defaults controller. Starts at the schema default so a
 * first frame drawn before defaults load matches what they will say.
 */
export const thread_title_mode = writable<ThreadTitleMode>(DefaultThreadTitleMode);

/**
 * The title a thread surface shows.
 *
 * A manual rename always wins — the lock is the reader's own word against any
 * derived name. Otherwise summary mode prefers the harness's generated session
 * title and falls back to the stored title (the latest user message) until an
 * engine has produced one; engines that never auto-name, Codex among them,
 * simply stay on the fallback.
 */
export const thread_display_title = (
	thread: {
		readonly summary_title?: string;
		readonly title: string;
		readonly title_locked: boolean;
	},
	mode: ThreadTitleMode,
): string =>
	mode === "summary" && !thread.title_locked && thread.summary_title !== undefined
		? thread.summary_title
		: thread.title;
