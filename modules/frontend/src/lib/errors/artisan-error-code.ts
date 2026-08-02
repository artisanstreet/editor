/** Stable user-visible error codes for Artisan frontend failures. */
export const ArtisanErrorCode = {
	CLIENT_CAPACITY_EXCEEDED: "CLIENT_CAPACITY_EXCEEDED",
	CLIENT_CONFIGURATION: "CLIENT_CONFIGURATION",
	CLIENT_DISPOSED: "CLIENT_DISPOSED",
	CLIENT_STATE_FAILURE: "CLIENT_STATE_FAILURE",
	CONNECTION_UNAVAILABLE: "CONNECTION_UNAVAILABLE",
	HYDRATION_PROJECTS_FAILED: "HYDRATION_PROJECTS_FAILED",
	HYDRATION_SESSION_DEFAULTS_FAILED: "HYDRATION_SESSION_DEFAULTS_FAILED",
	HYDRATION_THREADS_FAILED: "HYDRATION_THREADS_FAILED",
	PAIRING_REQUIRED: "PAIRING_REQUIRED",
	PROTOCOL_FAILURE: "PROTOCOL_FAILURE",
	TRANSPORT_MALFORMED: "TRANSPORT_MALFORMED",
} as const;

/** Identifies a stable, safe error that may be shown to an Artisan user. */
export type ArtisanErrorCode = (typeof ArtisanErrorCode)[keyof typeof ArtisanErrorCode];

const artisan_error_messages: Readonly<Record<ArtisanErrorCode, string>> = {
	CLIENT_CAPACITY_EXCEEDED: "Artisan is temporarily at capacity. Please try again.",
	CLIENT_CONFIGURATION: "Artisan needs local configuration before it can continue.",
	CLIENT_DISPOSED: "The Artisan connection was closed. Please reconnect.",
	CLIENT_STATE_FAILURE: "Artisan lost synchronization with the local service. Please reconnect.",
	CONNECTION_UNAVAILABLE: "Artisan could not reach the local Forge service.",
	HYDRATION_PROJECTS_FAILED: "Artisan could not load your projects.",
	HYDRATION_SESSION_DEFAULTS_FAILED: "Artisan could not load your session defaults.",
	HYDRATION_THREADS_FAILED: "Artisan could not load your threads.",
	PAIRING_REQUIRED: "This Artisan window needs to be paired with Forge.",
	PROTOCOL_FAILURE: "Artisan and Forge could not agree on their local protocol.",
	TRANSPORT_MALFORMED: "Artisan received an invalid local transport response.",
};

/** Formats one stable Artisan error without exposing a transport cause. */
export const format_artisan_error = (code: ArtisanErrorCode): string =>
	artisan_error_messages[code];
