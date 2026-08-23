import { Buffer } from "node:buffer";

import { Data, Effect } from "effect";
import WebSocket, { type RawData } from "ws";

const maximum_frame_bytes = 16 * 1024 * 1024;
const request_timeout_ms = 60_000;

export class HermesGatewayError extends Data.TaggedError("HermesGatewayError")<{
	readonly code: "closed" | "decode" | "network" | "protocol" | "remote" | "timeout";
	readonly message: string;
	readonly method?: string;
	readonly remote_code?: number;
}> {}

export interface HermesGatewayEvent {
	readonly payload?: unknown;
	readonly session_id?: string;
	readonly type: string;
}

export interface HermesGatewayClient {
	readonly Close: Effect.Effect<void>;
	readonly Closed: Effect.Effect<void>;
	readonly IsOpen: () => boolean;
	readonly Request: (
		method: string,
		params?: Readonly<Record<string, unknown>>,
	) => Effect.Effect<unknown, HermesGatewayError>;
	readonly Subscribe: (listener: (event: HermesGatewayEvent) => void) => () => void;
}

const record = (value: unknown): Readonly<Record<string, unknown>> | undefined =>
	typeof value === "object" && value !== null && !Array.isArray(value)
		? (value as Readonly<Record<string, unknown>>)
		: undefined;

const frame_bytes = (data: RawData) => {
	if (Buffer.isBuffer(data)) return data;
	if (Array.isArray(data)) return Buffer.concat(data);
	return Buffer.from(data);
};

interface PendingRequest {
	readonly method: string;
	readonly reject: (cause: HermesGatewayError) => void;
	readonly resolve: (value: unknown) => void;
	readonly timer: ReturnType<typeof setTimeout>;
}

class LiveHermesGatewayClient implements HermesGatewayClient {
	readonly #listeners = new Set<(event: HermesGatewayEvent) => void>();
	readonly #pending = new Map<number, PendingRequest>();
	readonly #ready: Promise<void>;
	readonly #socket: WebSocket;
	readonly #closed: Promise<void>;
	#next_id = 0;
	#settled = false;
	#resolve_closed!: () => void;
	#resolve_ready!: () => void;
	#reject_ready!: (cause: unknown) => void;

	constructor(socket: WebSocket) {
		this.#socket = socket;
		this.#ready = new Promise<void>((resolve, reject) => {
			this.#resolve_ready = resolve;
			this.#reject_ready = reject;
		});
		this.#closed = new Promise<void>((resolve) => {
			this.#resolve_closed = resolve;
		});
		socket.on("message", (data, is_binary) => this.#on_message(data, is_binary));
		socket.on("error", (cause) => this.#on_failure(cause));
		socket.on("close", () => this.#on_close());
	}

	readonly Closed = Effect.promise(() => this.#closed);

	readonly Close = Effect.sync(() => {
		if (this.#socket.readyState === WebSocket.CLOSED) return;
		if (this.#socket.readyState === WebSocket.CLOSING) return;
		this.#socket.close(1000, "Artisan closed the Hermes gateway");
	});

	IsOpen = () => this.#socket.readyState === WebSocket.OPEN && !this.#settled;

	Request = (method: string, params: Readonly<Record<string, unknown>> = {}) =>
		Effect.tryPromise({
			try: () =>
				new Promise<unknown>((resolve, reject) => {
					if (!this.IsOpen()) {
						reject(
							new HermesGatewayError({
								code: "closed",
								message: "Hermes gateway is not connected.",
								method,
							}),
						);
						return;
					}
					const id = ++this.#next_id;
					const timer = setTimeout(() => {
						this.#pending.delete(id);
						reject(
							new HermesGatewayError({
								code: "timeout",
								message: `Hermes did not answer ${method} within ${request_timeout_ms}ms.`,
								method,
							}),
						);
					}, request_timeout_ms);
					this.#pending.set(id, { method, reject, resolve, timer });
					this.#socket.send(
						JSON.stringify({ id, jsonrpc: "2.0", method, params }),
						(cause) => {
							if (cause == null) return;
							const pending = this.#pending.get(id);
							if (pending === undefined) return;
							clearTimeout(pending.timer);
							this.#pending.delete(id);
							pending.reject(
								new HermesGatewayError({
									code: "network",
									message: `Hermes ${method} could not be written.`,
									method,
								}),
							);
						},
					);
				}),
			catch: (cause) =>
				cause instanceof HermesGatewayError
					? cause
					: new HermesGatewayError({
							code: "network",
							message:
								cause instanceof Error ? cause.message : `Hermes ${method} failed.`,
							method,
						}),
		});

	Subscribe = (listener: (event: HermesGatewayEvent) => void) => {
		this.#listeners.add(listener);
		return () => this.#listeners.delete(listener);
	};

	WaitReady = () => this.#ready;

	#on_message(data: RawData, is_binary: boolean) {
		if (is_binary) {
			this.#on_failure(new Error("Hermes sent a binary JSON-RPC frame"));
			this.#socket.terminate();
			return;
		}
		const bytes = frame_bytes(data);
		if (bytes.byteLength > maximum_frame_bytes) {
			this.#on_failure(new Error("Hermes JSON-RPC frame exceeded 16 MiB"));
			this.#socket.terminate();
			return;
		}
		let value: unknown;
		try {
			value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
		} catch (cause) {
			this.#on_failure(cause);
			this.#socket.terminate();
			return;
		}
		const envelope = record(value);
		if (envelope === undefined) return;
		if (envelope.method === "event") {
			const params = record(envelope.params);
			if (typeof params?.type !== "string") return;
			const event: HermesGatewayEvent = {
				...(params.payload === undefined ? {} : { payload: params.payload }),
				...(typeof params.session_id === "string" ? { session_id: params.session_id } : {}),
				type: params.type,
			};
			if (event.type === "gateway.ready") this.#resolve_ready();
			for (const listener of this.#listeners) listener(event);
			return;
		}
		if (typeof envelope.id !== "number") return;
		const pending = this.#pending.get(envelope.id);
		if (pending === undefined) return;
		clearTimeout(pending.timer);
		this.#pending.delete(envelope.id);
		const remote_error = record(envelope.error);
		if (remote_error !== undefined) {
			pending.reject(
				new HermesGatewayError({
					code: "remote",
					message:
						typeof remote_error.message === "string"
							? remote_error.message
							: `Hermes rejected ${pending.method}.`,
					method: pending.method,
					...(typeof remote_error.code === "number"
						? { remote_code: remote_error.code }
						: {}),
				}),
			);
			return;
		}
		pending.resolve(envelope.result);
	}

	#on_failure(cause: unknown) {
		if (!this.#settled) this.#reject_ready(cause);
	}

	#on_close() {
		if (this.#settled) return;
		this.#settled = true;
		const failure = new HermesGatewayError({
			code: "closed",
			message: "Hermes gateway connection closed.",
		});
		this.#reject_ready(failure);
		for (const pending of this.#pending.values()) {
			clearTimeout(pending.timer);
			pending.reject(new HermesGatewayError({ ...failure, method: pending.method }));
		}
		this.#pending.clear();
		this.#listeners.clear();
		this.#resolve_closed();
	}
}

/** Connects to the private loopback gateway and waits for its protocol-ready event. */
export const ConnectHermesGateway = (
	endpoint: URL,
): Effect.Effect<HermesGatewayClient, HermesGatewayError> =>
	Effect.tryPromise({
		try: async () => {
			const socket = new WebSocket(endpoint, { maxPayload: maximum_frame_bytes });
			const client = new LiveHermesGatewayClient(socket);
			await new Promise<void>((resolve, reject) => {
				const opened = () => {
					cleanup();
					resolve();
				};
				const failed = (cause: unknown) => {
					cleanup();
					reject(cause);
				};
				const cleanup = () => {
					socket.off("open", opened);
					socket.off("error", failed);
					socket.off("close", failed);
				};
				socket.once("open", opened);
				socket.once("error", failed);
				socket.once("close", failed);
			});
			await client.WaitReady();
			return client;
		},
		catch: (cause) =>
			cause instanceof HermesGatewayError
				? cause
				: new HermesGatewayError({
						code: "network",
						message:
							cause instanceof Error
								? cause.message
								: "Hermes gateway connection failed.",
					}),
	}).pipe(
		Effect.timeoutOrElse({
			duration: "20 seconds",
			orElse: () =>
				Effect.fail(
					new HermesGatewayError({
						code: "timeout",
						message: "Hermes did not complete its gateway handshake in time.",
					}),
				),
		}),
	);
