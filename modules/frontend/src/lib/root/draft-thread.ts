import { writable } from "svelte/store";

import type { Project, ThreadSessionPolicy } from "@artisan/protocol";
import type { ComposerSubmission } from "$lib/composer/image-attachments";

/**
 * A new thread is a client-side draft until its first message is sent: no
 * durable thread exists yet, so the chosen project lives here and the thread
 * panel surfaces and edits it while the draft route is active.
 */
export const draft_thread_project = writable<Project | undefined>(undefined);

/**
 * The initial engine and model chosen before the durable thread exists.
 * Settled threads can later replace this policy through the same selector.
 */
export const draft_thread_policy = writable<ThreadSessionPolicy | undefined>(undefined);

/**
 * Hands the draft's first submission to the routed thread, which owns the
 * durable send pipeline. Consumed exactly once by the matching thread route.
 */
export const pending_first_submission = writable<
	| {
			readonly submission: ComposerSubmission;
			readonly thread_id: string;
	  }
	| undefined
>(undefined);
