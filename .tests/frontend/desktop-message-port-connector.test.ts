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
		this.listener?.(event);
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
						ports: ports.client_ports,
						source: host.self,
					});
					host.Dispatch({
						data: { generation: 1, type: DesktopConnectionMessageType },
						origin: host.origin,
						ports: [ports.client_ports[0]!],
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
