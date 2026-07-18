import {
	adapt_electron_renderer_message_port,
	DesktopSessionConnectionType,
	MessagePortConnector,
	MessagePortConnectorError,
} from "@artisan/transport/client";
import { Context, Effect, Layer, Option, PubSub, Queue, Ref, Schema, Stream } from "effect";

export const DesktopConnectionMessageType = DesktopSessionConnectionType;

export const FrontendConnectionPhase = Schema.Literals([
	"connecting",
	"error",
	"ready",
	"reconnecting",
	"stale",
	"unavailable",
]);

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
		typeof generation !== "number" ||
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

interface RendererWindowShape {
	readonly artisanDesktop?: {
		readonly requestConnection: () => void | Promise<void>;
	};
	readonly location: { readonly origin: string };
	readonly addEventListener: (
		event: "message",
		listener: (event: DesktopConnectionMessageEvent) => void,
	) => void;
	readonly removeEventListener: (
		event: "message",
		listener: (event: DesktopConnectionMessageEvent) => void,
	) => void;
}

const window_desktop_connection_host = (): Option.Option<DesktopConnectionHost> => {
	const renderer_window = (globalThis as { readonly window?: RendererWindowShape }).window;

	if (renderer_window === undefined || renderer_window.artisanDesktop === undefined) {
		return Option.none();
	}

	return Option.some({
		origin: renderer_window.location.origin,
		request_connection: () => renderer_window.artisanDesktop?.requestConnection(),
		self: renderer_window,
		add_message_listener: (listener) => renderer_window.addEventListener("message", listener),
		remove_message_listener: (listener) =>
			renderer_window.removeEventListener("message", listener),
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
							if (!Queue.offerUnsafe(messages, event)) {
								close_raw_ports(event.ports);
							}
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

							const ReceiveNextOffer = (): Effect.Effect<DesktopConnectionOffer> =>
								Effect.acquireUseRelease(
									Queue.take(messages).pipe(
										Effect.map((event) => ({ event, settled: false })),
									),
									(resource) =>
										Effect.gen(function* () {
											const offer = decode_offer(resource.event, host);

											if (Option.isNone(offer)) {
												close_raw_ports(resource.event.ports);
												resource.settled = true;

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
												close_raw_ports(resource.event.ports);
												resource.settled = true;

												return yield* ReceiveNextOffer();
											}

											resource.settled = true;

											return offer.value;
										}),
									(resource) =>
										Effect.sync(() => {
											if (!resource.settled) {
												close_raw_ports(resource.event.ports);
											}
										}),
								);
							const received = yield* ReceiveNextOffer().pipe(
								Effect.timeoutOption(`${connection_timeout_ms} millis`),
							);

							if (Option.isNone(received)) {
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

							const offer = received.value;
							const ports = yield* Effect.gen(function* () {
								const control_port = yield* adapt_electron_renderer_message_port(
									offer.control_port,
								);
								const stream_port = yield* adapt_electron_renderer_message_port(
									offer.stream_port,
								);

								return { control_port, stream_port };
							}).pipe(
								Effect.mapError(
									(cause) => new MessagePortConnectorError({ cause }),
								),
								Effect.tapError((error) =>
									Effect.gen(function* () {
										close_raw_ports([offer.control_port, offer.stream_port]);
										yield* SetState({
											message: String(error.cause),
											phase: "error",
										});
									}),
								),
							);

							yield* Ref.set(ever_connected, true);
							yield* SetState({ generation: offer.generation, phase: "ready" });

							return {
								control_port: ports.control_port,
								stream_port: ports.stream_port,
							};
						}),
					({ listener, messages }) =>
						Effect.gen(function* () {
							yield* Effect.sync(() => host.remove_message_listener(listener));
							const pending = yield* Queue.takeBetween(
								messages,
								0,
								Number.POSITIVE_INFINITY,
							);

							for (const event of pending) {
								close_raw_ports(event.ports);
							}

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
