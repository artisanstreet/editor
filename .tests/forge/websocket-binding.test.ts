import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import type { IncomingMessage } from "node:http";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";
import { WebSocket } from "ws";

import {
	BindForgeWebSocket,
	decode_forge_config,
	ForgeControlAuthority,
	make_forge_control_authority_layer,
	start_forge_http,
} from "../../modules/forge/src/index";

describe("Forge WebSocket binding lifecycle", () => {
	const closers: Array<() => Promise<void>> = [];

	afterEach(async () => {
		await Promise.all(closers.splice(0).map((close) => close()));
	});

	it("interrupts an active session during close without an unhandled rejection", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-forge-websocket-"));
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const config = decode_forge_config({
			database_path: join(directory, "artisan.sqlite"),
			instance_id: "websocket-lifecycle-test",
			migrations_path: join(directory, "migrations"),
		});
		const http = await Effect.runPromise(start_forge_http(config, authority));
		let session_released = false;
		const binding = await Effect.runPromise(
			BindForgeWebSocket({
				authority,
				config,
				http,
				ServeWebSocket: () =>
					Effect.never.pipe(
						Effect.ensuring(
							Effect.sync(() => {
								session_released = true;
							}),
						),
					),
			}),
		);
		closers.push(async () => {
			await Effect.runPromise(binding.Close.pipe(Effect.ignore));
			await Effect.runPromise(http.Close.pipe(Effect.ignore));
			await authority_runtime.dispose();
			await rm(directory, { force: true, recursive: true });
		});

		const session = await Effect.runPromise(
			authority.ConsumePair(await Effect.runPromise(authority.RequestPair)),
		);
		const websocket_url = new URL(config.websocket_path, http.endpoint);
		websocket_url.protocol = "ws:";
		const socket = new WebSocket(websocket_url, {
			headers: {
				cookie: `artisan_forge_session=${Option.getOrThrow(session)}`,
				origin: http.endpoint.origin,
			},
		});
		await once(socket, "open");

		const unhandled: Array<unknown> = [];
		const on_unhandled = (cause: unknown) => unhandled.push(cause);
		process.on("unhandledRejection", on_unhandled);
		try {
			const socket_closed = once(socket, "close");
			await Effect.runPromise(binding.Close);
			await socket_closed;
			await new Promise<void>((accept) => setImmediate(accept));
		} finally {
			process.off("unhandledRejection", on_unhandled);
		}

		expect(session_released).toBe(true);
		expect(unhandled).toEqual([]);
	});

	it("enforces the exact Host policy on Portless WebSocket upgrades", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-forge-portless-websocket-"));
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const config = decode_forge_config({
			allowed_hostnames: ["forge.localhost"],
			allowed_origins: ["https://editor.localhost"],
			database_path: join(directory, "artisan.sqlite"),
			instance_id: "portless-websocket-host-test",
			migrations_path: join(directory, "migrations"),
		});
		const http = await Effect.runPromise(start_forge_http(config, authority));
		const binding = await Effect.runPromise(
			BindForgeWebSocket({
				authority,
				config,
				http,
				ServeWebSocket: () => Effect.never,
			}),
		);
		closers.push(async () => {
			await Effect.runPromise(binding.Close.pipe(Effect.ignore));
			await Effect.runPromise(http.Close.pipe(Effect.ignore));
			await authority_runtime.dispose();
			await rm(directory, { force: true, recursive: true });
		});

		const session = await Effect.runPromise(
			authority.ConsumePair(await Effect.runPromise(authority.RequestPair)),
		);
		const websocket_url = new URL(config.websocket_path, http.endpoint);
		websocket_url.protocol = "ws:";
		const cookie = `artisan_forge_session=${Option.getOrThrow(session)}`;

		const rebound = new WebSocket(websocket_url, {
			headers: {
				cookie,
				host: "attacker.invalid",
				origin: "https://attacker.invalid",
			},
		});
		const [, rejected_response] = (await once(rebound, "unexpected-response")) as [
			unknown,
			IncomingMessage,
		];
		rejected_response.resume();
		expect(rejected_response.statusCode).toBe(403);

		const valid = new WebSocket(websocket_url, {
			headers: {
				cookie,
				host: "forge.localhost",
				origin: "https://editor.localhost",
			},
		});
		await once(valid, "open");
		const closed = once(valid, "close");
		valid.close();
		await closed;
	});

	it("resolves an endpoint send through the real ws success callback", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-forge-websocket-send-"));
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const config = decode_forge_config({
			database_path: join(directory, "artisan.sqlite"),
			instance_id: "websocket-send-test",
			migrations_path: join(directory, "migrations"),
		});
		const http = await Effect.runPromise(start_forge_http(config, authority));
		/**
		 * `ws` reports a successful send by invoking the callback with `null`.
		 * A strict `undefined` comparison once rejected every delivered frame,
		 * which killed each session immediately after transport negotiation.
		 */
		let serve_websocket!: (
			endpoint: Parameters<
				Parameters<typeof BindForgeWebSocket>[0]["ServeWebSocket"]
			>[0],
		) => Effect.Effect<void, unknown>;
		const delivered = new Promise<void>((accept, fail) => {
			const timer = setTimeout(() => fail(new Error("send never settled")), 10_000);
			serve_websocket = (endpoint) =>
				Effect.tryPromise(async () => {
					await endpoint.send(new Uint8Array([1, 2, 3]));
					clearTimeout(timer);
					accept();
				}).pipe(
					Effect.tapCause(() =>
						Effect.sync(() => {
							clearTimeout(timer);
							fail(new Error("send rejected for a delivered frame"));
						}),
					),
					Effect.andThen(Effect.never),
				);
		});
		const binding = await Effect.runPromise(
			BindForgeWebSocket({
				authority,
				config,
				http,
				ServeWebSocket: (endpoint) => serve_websocket(endpoint),
			}),
		);
		closers.push(async () => {
			await Effect.runPromise(binding.Close.pipe(Effect.ignore));
			await Effect.runPromise(http.Close.pipe(Effect.ignore));
			await authority_runtime.dispose();
			await rm(directory, { force: true, recursive: true });
		});

		const session = await Effect.runPromise(
			authority.ConsumePair(await Effect.runPromise(authority.RequestPair)),
		);
		const websocket_url = new URL(config.websocket_path, http.endpoint);
		websocket_url.protocol = "ws:";
		const socket = new WebSocket(websocket_url, {
			headers: { cookie: `artisan_forge_session=${Option.getOrThrow(session)}` },
		});
		const received = once(socket, "message");
		await once(socket, "open");

		await delivered;
		const [frame] = (await received) as [Buffer];
		expect([...frame]).toEqual([1, 2, 3]);
		socket.close();
	});

	it("destroys a pending upgrade when session authorization is interrupted", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-forge-upgrade-"));
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const config = decode_forge_config({
			database_path: join(directory, "artisan.sqlite"),
			instance_id: "websocket-upgrade-test",
			migrations_path: join(directory, "migrations"),
		});
		const http = await Effect.runPromise(start_forge_http(config, authority));
		let accept_authorization_started!: () => void;
		const authorization_started = new Promise<void>((accept) => {
			accept_authorization_started = accept;
		});
		const binding = await Effect.runPromise(
			BindForgeWebSocket({
				authority: {
					...authority,
					HasSession: () =>
						Effect.sync(accept_authorization_started).pipe(
							Effect.andThen(Effect.never),
						),
				},
				config,
				http,
				ServeWebSocket: () => Effect.never,
			}),
		);
		closers.push(async () => {
			await Effect.runPromise(binding.Close.pipe(Effect.ignore));
			await Effect.runPromise(http.Close.pipe(Effect.ignore));
			await authority_runtime.dispose();
			await rm(directory, { force: true, recursive: true });
		});

		const socket = createConnection({
			host: http.endpoint.hostname,
			port: Number(http.endpoint.port),
		});
		await once(socket, "connect");
		socket.write(
			[
				`GET ${config.websocket_path} HTTP/1.1`,
				`Host: ${http.endpoint.host}`,
				"Connection: Upgrade",
				"Upgrade: websocket",
				"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==",
				"Sec-WebSocket-Version: 13",
				"",
				"",
			].join("\r\n"),
		);
		await authorization_started;

		const socket_closed = once(socket, "close");
		await Effect.runPromise(binding.Close);
		await socket_closed;

		expect(socket.destroyed).toBe(true);
	});
});
