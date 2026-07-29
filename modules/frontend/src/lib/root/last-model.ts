import type { ThreadSessionPolicy } from "@artisan/protocol";

const storage_key = "artisan.last-model";

const storage = () => (globalThis as { readonly localStorage?: Storage }).localStorage;

/**
 * The native model id most recently chosen in any composer. A per-browser UI
 * preference that seeds new drafts; losing it (private mode, cleared storage)
 * only means falling back to the catalog default.
 */
export const remember_last_model = (policy: ThreadSessionPolicy) => {
	if (policy.model === undefined) return;
	try {
		storage()?.setItem(storage_key, policy.model);
	} catch {
		/* Unavailable storage must never block a model pick. */
	}
};

export const recall_last_model = (): string | undefined => {
	try {
		return storage()?.getItem(storage_key) ?? undefined;
	} catch {
		return undefined;
	}
};
