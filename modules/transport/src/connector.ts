import { Context, Data, Effect, Scope } from "effect";

import type { MessagePortLike } from "./message-port";

/** Reports why a client could not obtain a fresh pair of MessagePorts. */
export class MessagePortConnectorError extends Data.TaggedError("MessagePortConnectorError")<{
	readonly cause: unknown;
}> {}

/** Contains one independently buffered control and binary-stream port pair. */
export interface MessagePortConnection {
	readonly control_port: MessagePortLike;
	readonly stream_port: MessagePortLike;
}

/** Opens fresh scope-owned MessagePort pairs for initial connection and reconnect. */
export class MessagePortConnector extends Context.Service<
	MessagePortConnector,
	{
		readonly Connect: Effect.Effect<
			MessagePortConnection,
			MessagePortConnectorError,
			Scope.Scope
		>;
	}
>()("Artisan/MessagePortConnector") {}
