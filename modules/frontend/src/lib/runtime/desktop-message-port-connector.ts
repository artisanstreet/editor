import {
	adapt_electron_renderer_message_port,
	DesktopSessionConnectionType,
	MessagePortConnector,
	MessagePortConnectorError,
} from "@artisan/transport/client";
import { Context, Effect, Layer, Option, PubSub, Queue, Ref, Schema, Stream } from "effect";

export const DesktopConnectionMessageType = DesktopSessionConnectionType;

export const FrontendConnectionPhase = Schema.Literal(
	"connecting",
	"error",
	"ready",
	"reconnecting",
	"stale",
	"unavailable",
);

export type FrontendConnectionPhase = typeof FrontendConnectionPhase.Type;

export const FrontendConnectionState = Schema.Struct({
	generation: Schema.optional(Schema.Int),
	phase: FrontendConnectionPhase,
	message: Schema.optional(Schema.String),
});

export type FrontendConnectionState = typeof FrontendConnectionState.Type;

/** Renderer-owned observability for the shell connection, distinct from durable session projections. */
export class FrontendConnectionLifecycle extends Context.Service<
	FrontendConnectionLifecycle,
	{
		readonly Changes: Stream.Stream<FrontendConnectionState>;
		readonly Current: Effect.Effect<FrontendConnectionState>;
	}
>()("Artisan/FrontendConnectionLifecycle") {}

export interface DesktopConnectionMessageEvent {
	readonly data: unknown;
	readonly origin: string;
	readonly ports: ReadonlyArray<unknown>;
	readonly source: unknown;
}

export interface DesktopConnectionHost {
	readonly origin: string;
	readonly request_connection: () => void | Promise<void>;
	readonly self: unknown;
	readonly add_message_listener: (
		listener: (event: DesktopConnectionMessageEvent) => void,
	) => void;
	readonly remove_message_listener: (
		listener: (event: DesktopConnectionMessageEvent) => void,
	) => void;
}

export interface DesktopMessagePortConnectorOptions {
	readonly connection_timeout_ms?: number;
}

interface DesktopConnectionOffer {
	readonly control_port: Parameters<typeof adapt_electron_renderer_message_port>[0];
	readonly generation: number;
	readonly stream_port: Parameters<typeof adapt_electron_renderer_message_port>[0];
}

const is_renderer_message_port = (
	value: unknown,
): value is Parameters<typeof adapt_electron_renderer_message_port>[0] => {
	if (typeof value !== "object" || value === null) {
		return false;
	}

	return (
		"addEventListener" in value &&
		typeof value.addEventListener === "function" &&
		"close" in value &&
		typeof value.close === "function" &&
		"postMessage" in value &&
		typeof value.postMessage === "function" &&
		"removeEventListener" in value &&
		typeof value.removeEventListener === "function" &&
		"start" in value &&
		typeof value.start === "function"
	);
};

const decode_offer = (event: DesktopConnectionMessageEvent, host: DesktopConnectionHost) => {
	if (event.source !== host.self || event.origin !== host.origin) {
		return Option.none<DesktopConnectionOffer>();
	}

	if (typeof event.data !== "object" || event.data === null) {
		return Option.none<DesktopConnectionOffer>();
	}

	const payload = event.data as Record<string, unknown>;
	const generation = payload.generation;

	if (
		payload.type !== DesktopConnectionMessageType ||
		!Number.isSafeInteger(generation) ||
		generation < 1 ||
		event.ports.length !== 2
	) {
		return Option.none<DesktopConnectionOffer>();
	}

	const control_port = event.ports[0];
	const stream_port = event.ports[1];

	if (!is_renderer_message_port(control_port) || !is_renderer_message_port(stream_port)) {
		return Option.none<DesktopConnectionOffer>();
	}

	return Option.some({ control_port, generation, stream_port });
};

const close_raw_ports = (ports: ReadonlyArray<unknown>) => {
	for (const port of ports) {
		if (is_renderer_message_port(port)) {
			try {
				port.close();
			} catch {
				/** A transferred stale port may already be closed by the desktop shell. */
			}
		}
	}
};

const window_desktop_connection_host = (): Option.Option<DesktopConnectionHost> => {
	if (typeof window === "undefined" || window.artisanDesktop === undefined) {
		return Option.none();
	}

	return Option.some({
		origin: window.location.origin,
		request_connection: () => window.artisanDesktop?.requestConnection(),
		self: window,
		add_message_listener: (listener) => window.addEventListener("message", listener),
		remove_message_listener: (listener) => window.removeEventListener("message", listener),
	});
};

/**
 * Creates the scoped renderer connector for the one narrow desktop preload bridge.
 * MessagePorts are accepted only from the current renderer origin and only when their
 * generation moves forward, so delayed delivery cannot resurrect a stale session.
 */
export const make_frontend_message_port_connector_layer = (
	host: DesktopConnectionHost,
	options: DesktopMessagePortConnectorOptions = {},
) =>
	Layer.unwrap(
		Effect.gen(function* () {
			const connection_timeout_ms = options.connection_timeout_ms ?? 5_000;
			const current = yield* Ref.make<FrontendConnectionState>({ phase: "connecting" });
			const changes = yield* PubSub.sliding<FrontendConnectionState>(16);
			const ever_connected = yield* Ref.make(false);
			const last_generation = yield* Ref.make(0);

			const SetState = (state: FrontendConnectionState) =>
				Effect.gen(function* () {
					yield* Ref.set(current, state);
					yield* PubSub.publish(changes, state);
				});

			const lifecycle = FrontendConnectionLifecycle.of({
				Changes: Stream.fromPubSub(changes),
				Current: Ref.get(current),
			});
			const Connect = Effect.gen(function* () {
				const had_connection = yield* Ref.get(ever_connected);
				yield* SetState({ phase: had_connection ? "reconnecting" : "connecting" });

				return yield* Effect.acquireUseRelease(
					Effect.gen(function* () {
						const messages = yield* Queue.dropping<DesktopConnectionMessageEvent>(8);
						const listener = (event: DesktopConnectionMessageEvent) => {
							Queue.offerUnsafe(messages, event);
						};

						host.add_message_listener(listener);

						return { listener, messages };
					}),
					({ messages }) =>
						Effect.gen(function* () {
							yield* Effect.tryPromise({
								try: () => Promise.resolve(host.request_connection()),
								catch: (cause) => new MessagePortConnectorError({ cause }),
							}).pipe(
								Effect.tapError((error) =>
									SetState({ message: String(error.cause), phase: "error" }),
								),
							);

							const ReceiveNextOffer = (): Effect.Effect<
								DesktopConnectionOffer,
								MessagePortConnectorError
							> =>
								Effect.gen(function* () {
									const next = yield* Queue.take(messages).pipe(
										Effect.timeoutOption(`${connection_timeout_ms} millis`),
									);

									if (Option.isNone(next)) {
										const error = new MessagePortConnectorError({
											cause: new Error(
												"Timed out waiting for the desktop MessagePort pair.",
											),
										});

										yield* SetState({
											message: String(error.cause),
											phase: "error",
										});

										return yield* Effect.fail(error);
									}

									const offer = decode_offer(next.value, host);

									if (Option.isNone(offer)) {
										return yield* ReceiveNextOffer();
									}

									const accepted = yield* Ref.modify(
										last_generation,
										(generation) =>
											offer.value.generation > generation
												? ([true, offer.value.generation] as const)
												: ([false, generation] as const),
									);

									if (!accepted) {
										close_raw_ports(next.value.ports);

										return yield* ReceiveNextOffer();
									}

									return offer.value;
								});
							const offer = yield* ReceiveNextOffer();

							yield* Ref.set(ever_connected, true);
							yield* SetState({ generation: offer.generation, phase: "ready" });

							return {
								control_port: yield* adapt_electron_renderer_message_port(
									offer.control_port,
								).pipe(
									Effect.mapError(
										(cause) => new MessagePortConnectorError({ cause }),
									),
								),
								stream_port: yield* adapt_electron_renderer_message_port(
									offer.stream_port,
								).pipe(
									Effect.mapError(
										(cause) => new MessagePortConnectorError({ cause }),
									),
								),
							};
						}),
					({ listener, messages }) =>
						Effect.gen(function* () {
							yield* Effect.sync(() => host.remove_message_listener(listener));
							yield* Queue.shutdown(messages);
						}),
				);
			});

			return Layer.merge(
				Layer.succeed(MessagePortConnector, { Connect }),
				Layer.succeed(FrontendConnectionLifecycle, lifecycle),
			);
		}),
	);

/** Production connector: fixtures must opt into their own explicit client Layer. */
export const FrontendMessagePortConnectorLive = Layer.unwrap(
	Effect.gen(function* () {
		const host = window_desktop_connection_host();

		if (Option.isNone(host)) {
			return Layer.merge(
				Layer.succeed(MessagePortConnector, {
					Connect: Effect.fail(
						new MessagePortConnectorError({
							cause: new Error("The Artisan desktop bridge is unavailable."),
						}),
					),
				}),
				Layer.succeed(
					FrontendConnectionLifecycle,
					FrontendConnectionLifecycle.of({
						Changes: Stream.empty,
						Current: Effect.succeed({
							message: "The Artisan desktop bridge is unavailable.",
							phase: "unavailable",
						}),
					}),
				),
			);
		}

		return make_frontend_message_port_connector_layer(host.value);
	}),
);
