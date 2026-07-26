import { Effect } from "effect";

import type { WebSocketEndpoint } from "@artisan/transport/websocket/protocol";
import type { WebSocketPeer } from "@artisan/transport/websocket/server";

import type { ForgeConfig } from "./config";
import type { ForgeControlAuthorityShape } from "./control-authority";
import type { ForgeHttpServer } from "./http-host";

export interface ForgeTransportBindingInput {
	readonly config: ForgeConfig;
	readonly authority: ForgeControlAuthorityShape;
	readonly http: ForgeHttpServer;
	readonly ServeWebSocket: (
		endpoint: WebSocketEndpoint,
		peer: WebSocketPeer,
	) => Effect.Effect<void, unknown>;
}

export interface ForgeTransportHandle {
	readonly Close: Effect.Effect<void, unknown>;
}

export interface ForgeTransportBindingService {
	readonly Bind: (
		input: ForgeTransportBindingInput,
	) => Effect.Effect<ForgeTransportHandle, unknown>;
}

/** Useful for focused HTTP-host tests that deliberately omit the protocol. */
export const ForgeTransportBindingDisabled: ForgeTransportBindingService = {
	Bind: () => Effect.succeed({ Close: Effect.void }),
};
