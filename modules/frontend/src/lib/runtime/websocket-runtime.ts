import {
	make_websocket_connector_layer,
	type BrowserWebSocket,
} from "@artisan/transport/websocket/client";
import {
	MessagePortConnector,
	make_artisan_client_layer,
	TransportRuntimeLive,
} from "@artisan/transport/client";
import { Effect, Layer, PubSub, Ref, Stream } from "effect";

import {
	FrontendConnectionLifecycle,
	type FrontendConnectionState,
} from "./desktop-message-port-connector";

interface WebSocketDesktopBridge {
	readonly forgeWebSocketEndpoint?: unknown;
	readonly forgeWebSocketUrl?: unknown;
	readonly websocketUrl?: unknown;
}

export interface WebSocketRuntimeLocation {
	readonly origin: string;
	readonly protocol: string;
}

export interface WebSocketRuntimeTargetInput {
	readonly desktop?: WebSocketDesktopBridge;
	readonly development_url?: unknown;
	readonly is_development: boolean;
	readonly location?: WebSocketRuntimeLocation;
}

export type WebSocketRuntimeTarget =
	| { readonly _tag: "desktop" }
	| { readonly _tag: "unavailable" }
	| { readonly _tag: "websocket"; readonly url: string };

const ParseWebSocketUrl = (candidate: unknown) => {
	if (typeof candidate !== "string" || candidate.trim().length === 0) return undefined;

	try {
		const url = new URL(candidate);

		if (url.protocol === "ws:" || url.protocol === "wss:") return url.toString();
		if (url.protocol === "http:") {
			url.protocol = "ws:";
			return url.toString();
		}
		if (url.protocol === "https:") {
			url.protocol = "wss:";
			return url.toString();
		}
	} catch {
		/** An invalid optional endpoint falls through to the next real transport option. */
	}

	return undefined;
};

const BrowserWebSocketUrl = (location: WebSocketRuntimeLocation | undefined) => {
	if (
		location === undefined ||
		(location.protocol !== "http:" && location.protocol !== "https:")
	) {
		return undefined;
	}

	const url = new URL("/api/ws", location.origin);
	url.protocol = location.protocol === "https:" ? "wss:" : "ws:";

	return url.toString();
};

/**
 * Selects one real connection boundary. Browser development can opt into an explicit endpoint;
 * otherwise an http(s) page connects to its colocated Forge at `/api/ws`. The desktop
 * MessagePort remains the production fallback for installed builds that do not expose a socket.
 */
export const ResolveWebSocketRuntimeTarget = (
	input: WebSocketRuntimeTargetInput,
): WebSocketRuntimeTarget => {
	const development_url = input.is_development
		? ParseWebSocketUrl(input.development_url)
		: undefined;
	if (development_url !== undefined) return { _tag: "websocket", url: development_url };

	const desktop_url =
		ParseWebSocketUrl(input.desktop?.forgeWebSocketEndpoint) ??
		ParseWebSocketUrl(input.desktop?.forgeWebSocketUrl) ??
		ParseWebSocketUrl(input.desktop?.websocketUrl);
	if (desktop_url !== undefined) return { _tag: "websocket", url: desktop_url };

	const browser_url = BrowserWebSocketUrl(input.location);
	if (browser_url !== undefined) return { _tag: "websocket", url: browser_url };

	return input.desktop === undefined ? { _tag: "unavailable" } : { _tag: "desktop" };
};

const make_websocket_connection_lifecycle_layer = (
	url: string,
	create_socket?: (url: string) => BrowserWebSocket,
) =>
	Layer.unwrap(
		Effect.gen(function* () {
			const current = yield* Ref.make<FrontendConnectionState>({ phase: "connecting" });
			const changes = yield* PubSub.sliding<FrontendConnectionState>(16);
			const ever_connected = yield* Ref.make(false);
			const SetState = (state: FrontendConnectionState) =>
				Effect.gen(function* () {
					yield* Ref.set(current, state);
					yield* PubSub.publish(changes, state);
				});

			const socket_connector = make_websocket_connector_layer({
				...(create_socket === undefined ? {} : { create_socket }),
				url,
			});
			const observable_connector = Layer.effect(
				MessagePortConnector,
				Effect.gen(function* () {
					const connector = yield* MessagePortConnector;

					return {
						Connect: Effect.gen(function* () {
							const connected = yield* Ref.get(ever_connected);
							yield* SetState({ phase: connected ? "reconnecting" : "connecting" });
							const connection = yield* connector.Connect.pipe(
								Effect.tapError((error) =>
									SetState({ message: String(error.cause), phase: "error" }),
								),
							);
							yield* Ref.set(ever_connected, true);
							yield* SetState({ phase: "ready" });

							return connection;
						}),
					};
				}),
			).pipe(Layer.provide(socket_connector));

			return Layer.merge(
				observable_connector,
				Layer.succeed(
					FrontendConnectionLifecycle,
					FrontendConnectionLifecycle.of({
						Changes: Stream.fromPubSub(changes),
						Current: Ref.get(current),
					}),
				),
			);
		}),
	);

/** Provides the existing typed client and renderer lifecycle through a browser WebSocket. */
export const make_websocket_client_runtime_layer = (
	url: string,
	create_socket?: (url: string) => BrowserWebSocket,
) =>
	make_artisan_client_layer().pipe(
		Layer.provideMerge(make_websocket_connection_lifecycle_layer(url, create_socket)),
		Layer.provide(TransportRuntimeLive),
	);
