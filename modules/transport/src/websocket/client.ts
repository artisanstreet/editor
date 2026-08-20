import { Effect, Layer } from "effect";

import { MessagePortConnector, MessagePortConnectorError } from "../connector";
import { MakeWebSocketConnection, type WebSocketEndpoint } from "./protocol";

/** Browser-compatible WebSocket surface; injectable for local dev and tests. */
export interface BrowserWebSocket {
	readonly addEventListener: (event: string, listener: (event: Event) => void) => void;
	binaryType: "arraybuffer" | "blob";
	readonly close: () => void;
	readonly removeEventListener: (event: string, listener: (event: Event) => void) => void;
	readonly send: (data: ArrayBufferView) => void;
}

export interface WebSocketClientOptions {
	readonly create_socket?: (url: string) => BrowserWebSocket;
	readonly url: string | (() => string);
}

/** Makes browser binary messages decode as ArrayBuffer instead of Blob. */
export const prepare_browser_websocket = (socket: BrowserWebSocket) => {
	socket.binaryType = "arraybuffer";
	return socket;
};

const browser_endpoint = (socket: BrowserWebSocket): WebSocketEndpoint => ({
	add_close_listener: (listener) => {
		const callback = () => listener();
		socket.addEventListener("close", callback);

		return () => socket.removeEventListener("close", callback);
	},
	add_error_listener: (listener) => {
		const callback = (event: Event) => listener(event);
		socket.addEventListener("error", callback);

		return () => socket.removeEventListener("error", callback);
	},
	add_message_listener: (listener) => {
		const callback = (event: Event) => listener("data" in event ? event.data : undefined);
		socket.addEventListener("message", callback);

		return () => socket.removeEventListener("message", callback);
	},
	close: () => socket.close(),
	send: (data) => socket.send(data),
});

const AwaitOpen = (socket: BrowserWebSocket) =>
	Effect.callback<void, MessagePortConnectorError>((resume) => {
		let settled = false;
		const cleanup = () => {
			socket.removeEventListener("open", opened);
			socket.removeEventListener("error", failed);
			socket.removeEventListener("close", closed);
		};
		const complete = (effect: Effect.Effect<void, MessagePortConnectorError>) => {
			if (!settled) {
				settled = true;
				cleanup();
				resume(effect);
			}
		};
		const opened = () => complete(Effect.void);
		const failed = (cause: unknown) =>
			complete(Effect.fail(new MessagePortConnectorError({ cause })));
		const closed = () => failed(new Error("WebSocket closed before open"));

		socket.addEventListener("open", opened);
		socket.addEventListener("error", failed);
		socket.addEventListener("close", closed);

		return Effect.sync(() => {
			cleanup();
			socket.close();
		});
	});

/** Provides the existing reconnecting client with fresh two-channel connections over one socket. */
export const make_websocket_connector_layer = (options: WebSocketClientOptions) => {
	const create_socket = options.create_socket ?? ((url: string) => new WebSocket(url));
	const Connect = Effect.gen(function* () {
		const url = typeof options.url === "function" ? options.url() : options.url;
		const socket = yield* Effect.try({
			try: () => prepare_browser_websocket(create_socket(url)),
			catch: (cause) => new MessagePortConnectorError({ cause }),
		});

		yield* AwaitOpen(socket);
		const connection = yield* MakeWebSocketConnection(browser_endpoint(socket)).pipe(
			Effect.mapError((cause) => new MessagePortConnectorError({ cause })),
		);
		yield* Effect.addFinalizer(() => connection.control_port.Close);

		return connection;
	});

	return Layer.succeed(MessagePortConnector, { Connect });
};
