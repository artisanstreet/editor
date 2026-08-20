import type { IncomingMessage } from "node:http";

import { Cause, Data, Effect, Exit, FiberSet, Scope } from "effect";
import { WebSocket, WebSocketServer, type RawData } from "ws";

import type { WebSocketEndpoint } from "@artisan/transport/websocket/protocol";
import type { WebSocketPeer } from "@artisan/transport/websocket/server";

import { ForgeHostAllowed } from "./host-policy";
import type { ForgeTransportBindingInput, ForgeTransportHandle } from "./transport-binding";

export class ForgeWebSocketFailure extends Data.TaggedError("ForgeWebSocketFailure")<{
	readonly cause: unknown;
	readonly operation: "bind" | "close" | "session";
}> {}

const is_loopback = (address: string | undefined) =>
	address === "127.0.0.1" || address === "::1" || address === "::ffff:127.0.0.1";

/**
 * The unsent backlog a socket has to reach before it is worth saying so.
 *
 * `send` is fire-and-forget — the protocol layer above hands frames over
 * without being told whether the last one left — so a peer that reads slower
 * than Forge writes accumulates here with nothing to notice it. 8 MiB is far
 * above any legitimate burst of projection frames and far below the scale at
 * which this stopped being survivable, which makes it a usable alarm rather
 * than a limit: the frame is still sent, and the report is what turns a
 * process quietly climbing into a symptom with a cause attached.
 */
const socket_backlog_warning_bytes = 8 * 1024 * 1024;

const websocket_endpoint = (socket: WebSocket): WebSocketEndpoint => {
	let reported_backlog = false;

	return {
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
		send: (data) => {
			socket.send(data);
			if (socket.bufferedAmount < socket_backlog_warning_bytes) {
				reported_backlog = false;
				return;
			}
			/** Once per excursion: a backed-up socket would otherwise report per frame. */
			if (reported_backlog) return;
			reported_backlog = true;
			console.error(
				JSON.stringify({
					buffered_bytes: socket.bufferedAmount,
					event: "forge.websocket.backlog",
					message: "A transport socket is not draining as fast as Forge is writing",
				}),
			);
		},
	};
};

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
		const run_session = yield* FiberSet.makeRuntime().pipe(
			Effect.provideService(Scope.Scope, session_scope),
		);
		return yield* Effect.try({
			try: () => {
				const websocket_server = new WebSocketServer({
					maxPayload: 16 * 1024 * 1024,
					noServer: true,
					/**
					 * Compression is off, and the loopback-only rule this listener
					 * already enforces is the reason it can be.
					 *
					 * The bandwidth it bought was never real: every accepted socket
					 * comes from `127.0.0.1` — the upgrade handler rejects anything
					 * else with 403 — so the frames it shrank were about to be
					 * memcpy'd between two processes on the same machine. What it
					 * cost was: a zlib context per socket held open by context
					 * takeover, a compression pass over every frame above the
					 * threshold, and a queue in front of that pass.
					 *
					 * That queue is what makes this a correctness matter and not a
					 * tuning one. `concurrencyLimit` bounds how many frames compress
					 * at once, not how many wait, and `send` below hands frames over
					 * without ever asking whether the last one left. A projection
					 * burst that outruns zlib therefore accumulates compressed
					 * copies in native memory, where no heap limit and no collector
					 * can see them — which is the shape Forge kept growing into:
					 * 39 GB of private bytes in the one process that owns this
					 * server, spending CPU on compression the whole way up.
					 */
					perMessageDeflate: false,
				});
				const sockets = new Set<WebSocket>();
				const pending_upgrades = new Set<import("node:stream").Duplex>();

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
						!ForgeHostAllowed(request.headers.host, input.config) ||
						!ForgeOriginAllowed(request, input)
					) {
						write_upgrade_rejection(socket, 403);
						return;
					}
					pending_upgrades.add(socket);
					socket.once("close", () => pending_upgrades.delete(socket));
					run_session(ForgeSessionAllowed(request, input)).addObserver((exit) => {
						pending_upgrades.delete(socket);
						if (Exit.isFailure(exit)) {
							socket.destroy();
							return;
						}
						if (!exit.value) {
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
					run_session(
						input.ServeWebSocket(websocket_endpoint(socket), peer).pipe(
							Effect.catchCause((cause) =>
								Cause.hasInterruptsOnly(cause)
									? Effect.void
									: Effect.sync(() => {
											if (socket.readyState === WebSocket.OPEN) {
												socket.close(
													1011,
													"Artisan transport session failed",
												);
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
							for (const socket of pending_upgrades) {
								socket.destroy();
							}
							pending_upgrades.clear();
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
