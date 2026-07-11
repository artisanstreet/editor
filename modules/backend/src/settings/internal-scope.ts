/** Reserves a durable namespace for non-thread settings streams and command scopes. */
export const settings_scope_namespace = "settings/";

/** Returns the canonical durable scope for one global settings resource. */
export function settings_scope_id(resource: string) {
	return `${settings_scope_namespace}${resource}`;
}

/** Returns the canonical event-stream id for one global settings resource. */
export function settings_stream_id(resource: string) {
	return `settings:${resource}`;
}

/** Identifies ids that external thread creation must never claim. */
export function is_settings_scope_id(thread_id: string) {
	return thread_id.startsWith(settings_scope_namespace);
}
