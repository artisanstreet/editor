import { Data, Effect, Scope } from "effect";

import type { MessagePortConnection } from "../connector";
import { MakeWebSocketConnection, type WebSocketEndpoint } from "./protocol";

/** Host-provided peer identity is deliberately outside the generic transport. */
export interface WebSocketPeer {
	readonly is_loopback: boolean;
	readonly remote_address: string | undefined;
}

export class WebSocketAuthenticationError extends Data.TaggedError("WebSocketAuthenticationError")<{
	readonly cause: unknown;
}> {}

/** Lets a host apply loopback and token policy before protocol bytes are accepted. */
export interface WebSocketServerOptions {
	readonly authenticate?: (peer: WebSocketPeer) => Effect.Effect<void, unknown>;
	readonly incoming_capacity?: number;
	readonly require_loopback?: boolean;
}

/** Adapts one accepted socket to an existing MessagePort transport server binder. */
export const MakeWebSocketServerSession = (
	endpoint: WebSocketEndpoint,
	peer: WebSocketPeer,
	serve: (connection: MessagePortConnection) => Effect.Effect<void, unknown, Scope.Scope>,
	options: WebSocketServerOptions = {},
) =>
	Effect.gen(function* () {
		if ((options.require_loopback ?? true) && !peer.is_loopback) {
			return yield* Effect.fail(
				new WebSocketAuthenticationError({
					cause: new Error("non-loopback peer rejected"),
				}),
			);
		}

		if (options.authenticate) {
			yield* options
				.authenticate(peer)
				.pipe(Effect.mapError((cause) => new WebSocketAuthenticationError({ cause })));
		}

		const port_options =
			options.incoming_capacity === undefined
				? {}
				: { incoming_capacity: options.incoming_capacity };
		const connection = yield* MakeWebSocketConnection(endpoint, port_options);

		return yield* serve(connection);
	});
