import { describe, expect, it } from "vitest";
import { Effect, Fiber } from "effect";

import { ArtisanClient, MessagePortConnector } from "@artisan/transport/client";
import {
	DesktopConnectionMessageType,
	FrontendConnectionLifecycle,
	make_frontend_message_port_connector_layer,
	type DesktopConnectionHost,
	type DesktopConnectionMessageEvent,
} from "../../modules/frontend/src/lib/runtime/desktop-message-port-connector";
import { FrontendRuntimeLive } from "../../modules/frontend/src/lib/runtime/frontend-runtime";

class FakeDesktopHost {
	readonly self = {};
	readonly origin = "app://artisan";
	request_count = 0;
	active_listener_count = 0;
	private listener: ((event: DesktopConnectionMessageEvent) => void) | undefined;

	readonly as_host: DesktopConnectionHost = {
		add_message_listener: (listener) => {
			this.listener = listener;
			this.active_listener_count += 1;
		},
		origin: this.origin,
		remove_message_listener: (listener) => {
			if (this.listener === listener) {
				this.listener = undefined;
				this.active_listener_count -= 1;
			}
		},
		request_connection: () => {
			this.request_count += 1;
		},
		self: this.self,
	};

	Dispatch(event: DesktopConnectionMessageEvent) {
		if (this.listener === undefined) {
			return false;
		}

		this.listener(event);

		return true;
	}
}

class FakeRendererPort {
	close_count = 0;
	readonly throw_on_start: boolean;
	private readonly listeners = new Map<string, Set<(event: unknown) => void>>();

	constructor(throw_on_start = false) {
		this.throw_on_start = throw_on_start;
	}

	addEventListener(event: string, listener: (event: unknown) => void) {
		const listeners = this.listeners.get(event) ?? new Set();

		listeners.add(listener);
		this.listeners.set(event, listeners);
	}

	close() {
		this.close_count += 1;
	}

	postMessage(_message: unknown, _transfer?: ReadonlyArray<object>) {}

	removeEventListener(event: string, listener: (event: unknown) => void) {
		this.listeners.get(event)?.delete(listener);
	}

	start() {
		if (this.throw_on_start) {
			throw new Error("fake renderer port failed to start");
		}
	}
}

const MakePorts = () => {
	const control = new MessageChannel();
	const stream = new MessageChannel();

	return {
		client_ports: [control.port1, stream.port1],
		server_ports: [control.port2, stream.port2],
	};
};

describe("desktop renderer MessagePort connector", () => {
	it("keeps both the typed client and renderer lifecycle service in production composition", async () => {
		const services = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const client = yield* ArtisanClient;
					const lifecycle = yield* FrontendConnectionLifecycle;

					return { client, state: yield* lifecycle.Current };
				}),
			).pipe(Effect.provide(FrontendRuntimeLive)),
		);

		expect(services.client.Command).toBeTypeOf("function");
		expect(services.state).toMatchObject({ phase: "unavailable" });
	});

	it("accepts only a current-origin, forward-generation pair and scopes its ports", async () => {
		const host = new FakeDesktopHost();
		const ports = MakePorts();
		const wrong_origin_ports = [new FakeRendererPort(), new FakeRendererPort()];
		const malformed_ports = [new FakeRendererPort(), new FakeRendererPort()];
		const layer = make_frontend_message_port_connector_layer(host.as_host);

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const connector = yield* MessagePortConnector;
					const lifecycle = yield* FrontendConnectionLifecycle;
					const connecting = yield* lifecycle.Current;
					const pending = yield* connector.Connect.pipe(Effect.forkScoped);
					yield* Effect.yieldNow;

					host.Dispatch({
						data: { generation: 1, type: DesktopConnectionMessageType },
						origin: "https://untrusted.example",
						ports: wrong_origin_ports,
						source: host.self,
					});
					host.Dispatch({
						data: { generation: "invalid", type: DesktopConnectionMessageType },
						origin: host.origin,
						ports: malformed_ports,
						source: host.self,
					});
					host.Dispatch({
						data: { generation: 2, type: DesktopConnectionMessageType },
						origin: host.origin,
						ports: ports.client_ports,
						source: host.self,
					});

					const connection = yield* Fiber.join(pending);
					const ready = yield* lifecycle.Current;
					const active_listener_count = host.active_listener_count;
					const received = new Promise<string>((resolve) => {
						ports.server_ports[0]!.onmessage = (event) => resolve(String(event.data));
					});
					yield* connection.control_port.Send("connected");
					const server_message = yield* Effect.promise(() => received);

					return { active_listener_count, connecting, ready, server_message, connection };
				}),
			).pipe(Effect.provide(layer)),
		);

		expect(result.connecting).toEqual({ phase: "connecting" });
		expect(result.ready).toEqual({ generation: 2, phase: "ready" });
		expect(host.request_count).toBe(1);
		expect(result.active_listener_count).toBe(0);
		expect(result.connection.control_port).toBeDefined();
		expect(result.connection.stream_port).toBeDefined();
		expect(result.server_message).toBe("connected");
		expect(wrong_origin_ports.map((port) => port.close_count)).toEqual([1, 1]);
		expect(malformed_ports.map((port) => port.close_count)).toEqual([1, 1]);
	});

	it("never publishes ready when either transferred port fails acquisition", async () => {
		const host = new FakeDesktopHost();
		const control_port = new FakeRendererPort(true);
		const stream_port = new FakeRendererPort();
		const layer = make_frontend_message_port_connector_layer(host.as_host);

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const connector = yield* MessagePortConnector;
					const lifecycle = yield* FrontendConnectionLifecycle;
					const pending = yield* connector.Connect.pipe(Effect.exit, Effect.forkScoped);

					yield* Effect.yieldNow;
					host.Dispatch({
						data: { generation: 1, type: DesktopConnectionMessageType },
						origin: host.origin,
						ports: [control_port, stream_port],
						source: host.self,
					});

					return { exit: yield* Fiber.join(pending), state: yield* lifecycle.Current };
				}),
			).pipe(Effect.provide(layer)),
		);

		expect(result.exit._tag).toBe("Failure");
		expect(result.state.phase).toBe("error");
		expect(result.state.phase).not.toBe("ready");
		expect(control_port.close_count).toBeGreaterThanOrEqual(1);
		expect(stream_port.close_count).toBeGreaterThanOrEqual(1);
	});

	it("uses one deadline even while invalid offers keep arriving", async () => {
		const host = new FakeDesktopHost();
		const rejected_ports: Array<FakeRendererPort> = [];
		const layer = make_frontend_message_port_connector_layer(host.as_host, {
			connection_timeout_ms: 50,
		});

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const connector = yield* MessagePortConnector;
					const pending = yield* connector.Connect.pipe(Effect.exit, Effect.forkScoped);

					yield* Effect.yieldNow;
					let traffic_active = true;
					const interval = setInterval(() => {
						const ports = [new FakeRendererPort(), new FakeRendererPort()];

						const dispatched = host.Dispatch({
							data: { generation: 1, type: "not-artisan" },
							origin: host.origin,
							ports,
							source: host.self,
						});

						if (dispatched) {
							rejected_ports.push(...ports);
						}
					}, 2);
					const stop_traffic = setTimeout(() => {
						traffic_active = false;
						clearInterval(interval);
					}, 1_000);
					const exit = yield* Fiber.join(pending).pipe(
						Effect.ensuring(
							Effect.sync(() => {
								clearInterval(interval);
								clearTimeout(stop_traffic);
							}),
						),
					);

					return { exit, traffic_active_at_return: traffic_active };
				}),
			).pipe(Effect.provide(layer)),
		);

		expect(result.exit._tag).toBe("Failure");
		expect(result.traffic_active_at_return).toBe(true);
		expect(rejected_ports.length).toBeGreaterThan(2);
		expect(
			rejected_ports.every((port) => port.close_count >= 1),
			JSON.stringify(rejected_ports.map((port) => port.close_count)),
		).toBe(true);
	});

	it("removes the one-shot listener between reconnect generations", async () => {
		const host = new FakeDesktopHost();
		const first_ports = MakePorts();
		const second_ports = MakePorts();
		const layer = make_frontend_message_port_connector_layer(host.as_host);

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const connector = yield* MessagePortConnector;
					const lifecycle = yield* FrontendConnectionLifecycle;
					const first = yield* connector.Connect.pipe(Effect.forkScoped);

					yield* Effect.yieldNow;
					host.Dispatch({
						data: { generation: 1, type: DesktopConnectionMessageType },
						origin: host.origin,
						ports: first_ports.client_ports,
						source: host.self,
					});
					yield* Fiber.join(first);
					const after_first = host.active_listener_count;
					const second = yield* connector.Connect.pipe(Effect.forkScoped);

					yield* Effect.yieldNow;
					host.Dispatch({
						data: { generation: 2, type: DesktopConnectionMessageType },
						origin: host.origin,
						ports: second_ports.client_ports,
						source: host.self,
					});
					yield* Fiber.join(second);

					return {
						after_first,
						after_second: host.active_listener_count,
						state: yield* lifecycle.Current,
					};
				}),
			).pipe(Effect.provide(layer)),
		);

		expect(result.after_first).toBe(0);
		expect(result.after_second).toBe(0);
		expect(result.state).toEqual({ generation: 2, phase: "ready" });
		expect(host.request_count).toBe(2);
	});
});
