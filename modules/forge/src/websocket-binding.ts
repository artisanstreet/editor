import type { IncomingMessage } from "node:http";

import { Data, Effect, Exit, FiberSet, Scope } from "effect";
import { WebSocket, WebSocketServer, type RawData } from "ws";

import type { WebSocketEndpoint } from "@artisan/transport/websocket/protocol";
import type { WebSocketPeer } from "@artisan/transport/websocket/server";

import type { ForgeTransportBindingInput, ForgeTransportHandle } from "./transport-binding";

export class ForgeWebSocketFailure extends Data.TaggedError("ForgeWebSocketFailure")<{
	readonly cause: unknown;
	readonly operation: "bind" | "close" | "session";
}> {}

const is_loopback = (address: string | undefined) =>
	address === "127.0.0.1" || address === "::1" || address === "::ffff:127.0.0.1";

const websocket_endpoint = (socket: WebSocket): WebSocketEndpoint => ({
	add_close_listener: (listener) => {
		socket.on("close", listener);

		return () => socket.off("close", listener);
	},
	add_error_listener: (listener) => {
		socket.on("error", listener);

		return () => socket.off("error", listener);
	},
	add_message_listener: (listener) => {
		const on_message = (data: RawData) => listener(data);
		socket.on("message", on_message);

		return () => socket.off("message", on_message);
	},
	close: () => socket.close(),
	send: (data) => socket.send(data),
});

const request_url = (request: IncomingMessage) =>
	new URL(request.url ?? "/", "http://artisan.invalid");

const write_upgrade_rejection = (socket: import("node:stream").Duplex, status: 401 | 403 | 404) => {
	const reason = status === 401 ? "Unauthorized" : status === 403 ? "Forbidden" : "Not Found";
	socket.end(`HTTP/1.1 ${status} ${reason}\r\nConnection: close\r\n\r\n`);
};

export const ForgeOriginAllowed = (request: IncomingMessage, input: ForgeTransportBindingInput) => {
	const origin = request.headers.origin;

	if (origin === undefined) {
		return true;
	}

	try {
		return (
			new URL(origin).host === request.headers.host ||
			input.config.allowed_origins.includes(origin)
		);
	} catch {
		return false;
	}
};

export const ForgeSessionAllowed = (
	request: IncomingMessage,
	input: ForgeTransportBindingInput,
) => {
	const session = (request.headers.cookie ?? "")
		.split(";")
		.find((entry) => entry.trim().startsWith("artisan_forge_session="))
		?.split("=")
		.slice(1)
		.join("=");
	return input.authority.HasSession(session);
};

/**
 * Binds the existing reliable Artisan protocol server to one multiplexed
 * WebSocket without giving HTTP ownership to the backend domain layer.
 */
export const BindForgeWebSocket = (
	input: ForgeTransportBindingInput,
): Effect.Effect<ForgeTransportHandle, ForgeWebSocketFailure> =>
	Effect.gen(function* () {
		const session_scope = yield* Scope.make();
		const run_session = yield* FiberSet.makeRuntimePromise().pipe(
			Effect.provideService(Scope.Scope, session_scope),
		);
		return yield* Effect.try({
			try: () => {
				const websocket_server = new WebSocketServer({
					maxPayload: 16 * 1024 * 1024,
					noServer: true,
				});
				const sockets = new Set<WebSocket>();

				const on_upgrade = (
					request: IncomingMessage,
					socket: import("node:stream").Duplex,
					head: Buffer,
				) => {
					if (request_url(request).pathname !== input.config.websocket_path) {
						write_upgrade_rejection(socket, 404);
						return;
					}
					if (
						!is_loopback(request.socket.remoteAddress) ||
						!ForgeOriginAllowed(request, input)
					) {
						write_upgrade_rejection(socket, 403);
						return;
					}
					void run_session(ForgeSessionAllowed(request, input)).then((session) => {
						if (!session) {
							write_upgrade_rejection(socket, 401);
							return;
						}
						websocket_server.handleUpgrade(request, socket, head, (websocket) => {
							websocket_server.emit("connection", websocket, request);
						});
					});
				};

				input.http.node_server.on("upgrade", on_upgrade);
				websocket_server.on("connection", (socket, request) => {
					sockets.add(socket);
					socket.once("close", () => sockets.delete(socket));
					const peer: WebSocketPeer = {
						is_loopback: is_loopback(request.socket.remoteAddress),
						remote_address: request.socket.remoteAddress,
					};
					void run_session(
						input.ServeWebSocket(websocket_endpoint(socket), peer).pipe(
							Effect.catchCause((cause) =>
								Effect.sync(() => {
									if (socket.readyState === WebSocket.OPEN) {
										socket.close(1011, "Artisan transport session failed");
									}
									console.error(
										JSON.stringify({
											kind: "artisan:forge-session-failed",
											message: String(cause),
										}),
									);
								}),
							),
						),
					);
				});

				const Close = Effect.gen(function* () {
					yield* Scope.close(session_scope, Exit.void);
					yield* Effect.tryPromise({
						catch: (cause) => new ForgeWebSocketFailure({ cause, operation: "close" }),
						try: async () => {
							input.http.node_server.off("upgrade", on_upgrade);
							for (const socket of sockets) {
								socket.close(1001, "Artisan Forge is stopping");
							}
							await new Promise<void>((accept) =>
								websocket_server.close(() => accept()),
							);
						},
					});
				});

				return { Close };
			},
			catch: (cause) => new ForgeWebSocketFailure({ cause, operation: "bind" }),
		});
	});
