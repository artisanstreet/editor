import { Context, Data, Effect } from "effect";

import type { MessagePortConnection } from "./connector";

/** Identifies a MessagePort server-session failure. */
export type MessagePortTransportServerErrorCode =
	| "bootstrap"
	| "configuration"
	| "correlation_conflict"
	| "malformed"
	| "port"
	| "stale_connection";

/** Reports a typed failure while one port pair serves a protocol connection. */
export class MessagePortTransportServerError extends Data.TaggedError(
	"MessagePortTransportServerError",
)<{
	readonly cause: unknown;
	readonly code: MessagePortTransportServerErrorCode;
}> {}

/** Configures logical stream concurrency. */
export interface MessagePortTransportServerOptions {
	readonly max_active_streams?: number;
}

/** Binds one control port and one isolated binary port to a ProtocolConnection. */
export interface MessagePortTransportServerShape {
	readonly Serve: (
		ports: MessagePortConnection,
	) => Effect.Effect<void, MessagePortTransportServerError>;
}

/** Binds one control port and one isolated binary port to a ProtocolConnection. */
export class MessagePortTransportServer extends Context.Service<
	MessagePortTransportServer,
	MessagePortTransportServerShape
>()("Artisan/MessagePortTransportServer") {}
