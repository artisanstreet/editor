import {
	make_websocket_connector_layer,
	type BrowserWebSocket,
} from "@artisan/transport/websocket/client";
import {
	MessagePortConnector,
	MessagePortConnectorError,
	make_artisan_client_layer,
	TransportRuntimeLive,
} from "@artisan/transport/client";
import { Effect, Layer } from "effect";

export interface WebSocketRuntimeLocation {
	readonly origin: string;
	readonly protocol: string;
}

export interface WebSocketRuntimeTargetInput {
	readonly development_url?: unknown;
	/**
	 * Adopted loopback Forge origin for the installed editor's app-scheme
	 * renderer, whose page origin carries no usable Forge address of its own.
	 */
	readonly forge_endpoint?: string;
	readonly is_development: boolean;
	readonly location?: WebSocketRuntimeLocation;
}

export type WebSocketRuntimeTarget =
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
 * Selects the browser's only real connection boundary. Development can opt into
 * an explicit endpoint; the installed editor supplies its adopted loopback
 * Forge; otherwise an HTTP(S) page connects to its colocated Forge.
 */
export const ResolveWebSocketRuntimeTarget = (
	input: WebSocketRuntimeTargetInput,
): WebSocketRuntimeTarget => {
	const development_url = input.is_development
		? ParseWebSocketUrl(input.development_url)
		: undefined;
	if (development_url !== undefined) return { _tag: "websocket", url: development_url };

	const forge_endpoint_url =
		input.forge_endpoint === undefined
			? undefined
			: ParseWebSocketUrl(`${input.forge_endpoint.replace(/\/+$/, "")}/api/ws`);
	if (forge_endpoint_url !== undefined) return { _tag: "websocket", url: forge_endpoint_url };

	const browser_url = BrowserWebSocketUrl(input.location);
	if (browser_url !== undefined) return { _tag: "websocket", url: browser_url };

	return { _tag: "unavailable" };
};

const make_websocket_connector_runtime_layer = (
	url: string,
	create_socket?: (url: string) => BrowserWebSocket,
) =>
	make_websocket_connector_layer({
		...(create_socket === undefined ? {} : { create_socket }),
		url,
	});

const UnavailableWebSocketConnectorLive = Layer.succeed(
	MessagePortConnector,
	MessagePortConnector.of({
		Connect: Effect.gen(function* () {
			return yield* Effect.fail(
				new MessagePortConnectorError({
					cause: new Error("Artisan Forge WebSocket endpoint is unavailable."),
				}),
			);
		}),
	}),
);

/** Provides the existing typed client and renderer lifecycle through a browser WebSocket. */
export const make_websocket_client_runtime_layer = (
	target: WebSocketRuntimeTarget,
	create_socket?: (url: string) => BrowserWebSocket,
) =>
	/**
	 * Fail fast on an unreachable Forge: three quick attempts, then surface the
	 * gate's recovery controls. A loopback refusal is immediate, so waiting out
	 * a long exponential ladder only delays the Start/Retry affordances.
	 */
	make_artisan_client_layer({ reconnect_attempts: 3, reconnect_delay_ms: 250 }).pipe(
		Layer.provideMerge(
			target._tag === "websocket"
				? make_websocket_connector_runtime_layer(target.url, create_socket)
				: UnavailableWebSocketConnectorLive,
		),
		Layer.provide(TransportRuntimeLive),
	);
