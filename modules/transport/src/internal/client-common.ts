import { Effect } from "effect";

import type {
	ControlRpcRequest,
	ControlRpcSuccess,
	InboundControlEnvelope,
	ProtocolErrorDetail,
} from "@artisan/protocol";

import {
	ArtisanClientError,
	type ArtisanClientErrorCode,
	type ArtisanClientOptions,
} from "../client-api/service";
import type { MessagePortConnection } from "../connector";
import type { MessagePortError } from "../message-port";

/** Carries the currently negotiated ids and isolated port pair. */
export interface ActiveClientSession {
	readonly connection_id: string;
	readonly ports: MessagePortConnection;
	readonly protocol_connection_id: string;
	readonly stream_ticket: string;
}

/** Sends one already-built envelope if a connection is currently ready. */
export type SendCurrent = (
	envelope: InboundControlEnvelope,
) => Effect.Effect<void, ArtisanClientError>;

/**
 * Whether a live session actually carried an envelope onto the wire. A held
 * envelope waits for the next session to resend it, so nothing on the backend
 * has seen the request it describes and no result will ever correlate to it.
 */
export type RequestDelivery = { readonly _tag: "Delivered" } | { readonly _tag: "Held" };

export const RequestDelivered: RequestDelivery = { _tag: "Delivered" };
export const RequestHeld: RequestDelivery = { _tag: "Held" };

/** Sends one already-built envelope and reports whether a session carried it. */
export type SendRequest = (
	envelope: InboundControlEnvelope,
) => Effect.Effect<RequestDelivery, ArtisanClientError>;

/** Waits for the current negotiated session without exposing connection state. */
export type AwaitActive = Effect.Effect<ActiveClientSession, ArtisanClientError>;

/** Supplies one frontend trace reused when building a single envelope. */
export interface FrontendTrace {
	readonly message_id: string;
	readonly origin: "frontend";
	readonly protocol_version: 1;
	readonly schema_version: 1;
	readonly sent_at: string;
}

/** Creates a fresh trace once; reconnect retries retain the completed envelope. */
export type MakeTrace = Effect.Effect<FrontendTrace>;

export type PendingRequestEnvelope = ControlRpcRequest;
export type PendingResultEnvelope = ControlRpcSuccess;

/** Creates one consistently shaped typed client failure. */
export function client_error(
	code: ArtisanClientErrorCode,
	message: string,
	cause: unknown,
	retryable = false,
	protocol_code = "",
) {
	return new ArtisanClientError({ cause, code, message, protocol_code, retryable });
}

/** Converts a provider-neutral protocol error into the client error channel. */
export function protocol_client_error(detail: ProtocolErrorDetail, cause: unknown = detail) {
	return client_error("protocol", detail.message, cause, detail.retryable, detail.code);
}

/** Converts a normalized port failure into a retryable connection error. */
export function map_port_error(cause: MessagePortError) {
	return client_error("connection", "The MessagePort connection closed.", cause, true);
}

/** Produces deterministic stream cursor ordering for hello and ACK envelopes. */
export function record_to_cursors(cursors: Readonly<Record<string, number>>) {
	return Object.entries(cursors)
		.map(([stream_id, sequence]) => ({ sequence, stream_id }))
		.sort((left, right) => left.stream_id.localeCompare(right.stream_id));
}

/** Converts an ordered protocol cursor list into the client's lookup representation. */
export function cursors_to_record(
	cursors: ReadonlyArray<{ readonly sequence: number; readonly stream_id: string }>,
) {
	return Object.fromEntries(
		cursors.map(({ sequence, stream_id }) => [stream_id, sequence]),
	) as Readonly<Record<string, number>>;
}

/** Validates all client limits before any scoped fibers or queues are acquired. */
export function validate_client_options(options: Required<ArtisanClientOptions>) {
	return (
		Number.isSafeInteger(options.error_capacity) &&
		options.error_capacity > 0 &&
		Number.isSafeInteger(options.event_capacity) &&
		options.event_capacity > 0 &&
		Number.isSafeInteger(options.max_pending_requests) &&
		options.max_pending_requests > 0 &&
		Number.isSafeInteger(options.reconnect_attempts) &&
		options.reconnect_attempts > 0 &&
		Number.isSafeInteger(options.reconnect_delay_ms) &&
		options.reconnect_delay_ms >= 0 &&
		Number.isSafeInteger(options.stream_capacity) &&
		options.stream_capacity > 0 &&
		Number.isSafeInteger(options.subscription_capacity) &&
		options.subscription_capacity > 0
	);
}
