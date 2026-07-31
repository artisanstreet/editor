import http, { type IncomingMessage } from "node:http";
import { mkdtemp, mkdir, readFile, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { Effect, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	acquire_forge_database_lease,
	ForgeDatabaseAlreadyOwned,
	decode_forge_config,
	ForgeControlAuthority,
	ForgeOriginAllowed,
	ForgeSessionAllowed,
	make_forge_control_authority_layer,
	RemoveForgeState,
	start_forge_http,
	WriteForgeState,
} from "../../modules/forge/src/index";
import type { ForgeTransportBindingInput } from "../../modules/forge/src/transport-binding";

const test_instance_id = "2ef3d1c0-e8a4-4f4d-9d8a-744b1f18879d";

describe("Forge boundary", () => {
	const closers: Array<() => Promise<void>> = [];

	afterEach(async () => {
		await Promise.all(closers.splice(0).map((close) => close()));
	});

	it("schema-decodes a loopback-only default listener", () => {
		const config = decode_forge_config({
			database_path: "C:/artisan/data.sqlite",
			instance_id: test_instance_id,
			migrations_path: "C:/artisan/migrations",
		});

		expect(config.listen_host).toBe("127.0.0.1");
		expect(config.listen_port).toBe(0);
	});

	it("rejects a second owner for the same durable database", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-forge-"));
		const database_path = join(directory, "artisan.sqlite");
		const first = await Effect.runPromise(acquire_forge_database_lease(database_path));
		closers.push(() => Effect.runPromise(first.Release));

		const exit = await Effect.runPromise(
			acquire_forge_database_lease(database_path).pipe(Effect.exit),
		);

		expect(exit._tag).toBe("Failure");
		if (exit._tag === "Failure") {
			expect(String(exit.cause)).toContain(ForgeDatabaseAlreadyOwned.name);
		}
	});

	it("serves health and optional static frontend assets without Electron", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-forge-"));
		const static_root = join(directory, "web");
		await mkdir(static_root);
		await writeFile(join(static_root, "index.html"), "<main>Artisan</main>", "utf8");
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const host = await Effect.runPromise(
			start_forge_http(
				decode_forge_config({
					database_path: join(directory, "artisan.sqlite"),
					instance_id: test_instance_id,
					migrations_path: join(directory, "migrations"),
					static_frontend_root: static_root,
				}),
				authority,
			),
		);
		closers.push(async () => {
			await Effect.runPromise(host.Close);
			await authority_runtime.dispose();
		});

		const health = await fetch(new URL("/healthz", host.endpoint));
		/**
		 * Static hosting no longer implies development: the marker is stated by
		 * whoever launched the instance, and this composition states nothing.
		 */
		expect(await health.json()).toEqual({
			development: false,
			service: "artisan-forge",
			status: "ready",
			version: 1,
		});
		const asset = await fetch(host.endpoint);
		expect(await asset.text()).toBe("<main>Artisan</main>");
		const deep_link = await fetch(new URL("/t/workspace_1/thread_1", host.endpoint));
		expect(deep_link.status).toBe(200);
		expect(await deep_link.text()).toBe("<main>Artisan</main>");
		const deep_link_head = await fetch(new URL("/t/workspace_1/thread_1", host.endpoint), {
			method: "HEAD",
		});
		expect(deep_link_head.status).toBe(200);
		expect(deep_link_head.headers.get("content-type")).toBe("text/html; charset=utf-8");
		expect(await deep_link_head.text()).toBe("");
		const instances = await fetch(new URL("/api/instances", host.endpoint));
		expect(await instances.json()).toEqual({ instances: [] });
		const foreign_instances = await fetch(new URL("/api/instances", host.endpoint), {
			headers: { origin: "https://attacker.invalid" },
		});
		expect(foreign_instances.status).toBe(403);
		/**
		 * DNS rebinding presents an attacker Host name to the loopback
		 * listener. fetch() strips a caller-supplied Host header, so the probe
		 * speaks raw HTTP.
		 */
		const rebound_status = (pathname: string) =>
			new Promise<number>((accept, reject) => {
				const probe = http.request(
					{
						headers: { host: "attacker.invalid:4849" },
						host: host.endpoint.hostname,
						path: pathname,
						port: host.endpoint.port,
					},
					(response) => {
						response.resume();
						accept(response.statusCode ?? 0);
					},
				);
				probe.once("error", reject);
				probe.end();
			});
		expect(await rebound_status("/health")).toBe(403);
		expect(await rebound_status("/api/instances")).toBe(403);
		expect((await fetch(new URL("/api/unknown", host.endpoint))).status).toBe(404);
		expect((await fetch(new URL("/_app/unknown", host.endpoint))).status).toBe(404);
		expect((await fetch(new URL("/_app/does-not-exist.js", host.endpoint))).status).toBe(404);
		expect((await fetch(new URL("/missing.js", host.endpoint))).status).toBe(404);
		expect((await fetch(new URL("/%ZZ", host.endpoint))).status).toBe(400);
		const outside = join(directory, "outside");
		await mkdir(outside);
		await writeFile(join(outside, "secret.txt"), "not public", "utf8");
		await symlink(outside, join(static_root, "escape"), "junction");
		expect((await fetch(new URL("/escape/secret.txt", host.endpoint))).status).toBe(403);
	});

	it("mints one-time browser sessions while reserving shutdown for the control bearer", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-forge-control-"));
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const host = await Effect.runPromise(
			start_forge_http(
				decode_forge_config({
					auth_token: "forge-control-token-with-at-least-32-characters",
					database_path: join(directory, "artisan.sqlite"),
					instance_id: test_instance_id,
					migrations_path: join(directory, "migrations"),
				}),
				authority,
			),
		);
		closers.push(async () => {
			await Effect.runPromise(host.Close);
			await authority_runtime.dispose();
		});

		expect(
			(await fetch(new URL("/api/pair/request", host.endpoint), { method: "POST" })).status,
		).toBe(401);
		expect((await fetch(new URL("/api/control/status", host.endpoint))).status).toBe(401);
		const status = await fetch(new URL("/api/control/status", host.endpoint), {
			headers: {
				authorization: "Bearer forge-control-token-with-at-least-32-characters",
			},
		});
		expect(await status.json()).toMatchObject({
			instance_id: test_instance_id,
			service: "artisan-forge",
			status: "ready",
			version: 1,
		});
		const request = await fetch(new URL("/api/pair/request", host.endpoint), {
			headers: {
				authorization: "Bearer forge-control-token-with-at-least-32-characters",
			},
			method: "POST",
		});
		const code = ((await request.json()) as { readonly code: string }).code;
		const paired = await fetch(new URL("/api/pair", host.endpoint), {
			body: JSON.stringify({ code }),
			headers: { "content-type": "application/json" },
			method: "POST",
		});
		expect(paired.status).toBe(200);
		const cookie = paired.headers.get("set-cookie");
		expect(cookie).toContain("artisan_forge_session=");
		expect(cookie).toContain("HttpOnly");
		expect(cookie).toContain("SameSite=Strict");
		expect(
			(
				await fetch(new URL("/api/pair", host.endpoint), {
					body: JSON.stringify({ code }),
					headers: { "content-type": "application/json" },
					method: "POST",
				})
			).status,
		).toBe(401);
		expect(
			(
				await fetch(new URL("/api/control/shutdown", host.endpoint), {
					headers: { cookie: cookie! },
					method: "POST",
				})
			).status,
		).toBe(401);
		expect(
			(
				await fetch(new URL("/api/control/shutdown", host.endpoint), {
					headers: {
						authorization: "Bearer forge-control-token-with-at-least-32-characters",
					},
					method: "POST",
				})
			).status,
		).toBe(202);
		await Effect.runPromise(authority.ShutdownRequested);
	});

	it("expires pairing codes using its injected clock", async () => {
		let now = 0;
		const runtime = ManagedRuntime.make(make_forge_control_authority_layer({ now: () => now }));
		const authority = await runtime.runPromise(ForgeControlAuthority);
		const code = await Effect.runPromise(authority.RequestPair);
		now = 60_001;
		expect((await Effect.runPromise(authority.ConsumePair(code)))._tag).toBe("None");
		await runtime.dispose();
	});

	it("writes atomic non-secret state and preserves another instance on cleanup", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-forge-state-"));
		const path = join(directory, "forge.json");
		await Effect.runPromise(
			WriteForgeState(path, {
				endpoint: "http://127.0.0.1:4848/",
				instance_id: "instance-a",
				pid: 42,
				started_at: "2026-07-26T00:00:00.000Z",
				version: 1,
			}).pipe(Effect.provide(MakeSnowflakeIdLive(37))),
		);
		expect(JSON.parse(await readFile(path, "utf8"))).toMatchObject({
			instance_id: "instance-a",
			version: 1,
		});
		await Effect.runPromise(RemoveForgeState(path, "instance-b"));
		expect(JSON.parse(await readFile(path, "utf8"))).toMatchObject({
			instance_id: "instance-a",
		});
		await Effect.runPromise(RemoveForgeState(path, "instance-a"));
		await expect(readFile(path, "utf8")).rejects.toMatchObject({ code: "ENOENT" });
	});

	it("accepts session cookies and compares browser origins against the forwarded Host", async () => {
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const session = await Effect.runPromise(
			authority.ConsumePair(await Effect.runPromise(authority.RequestPair)),
		);
		const session_token = Option.getOrThrow(session);
		const input = {
			authority,
			config: decode_forge_config({
				database_path: "C:/forge.sqlite",
				instance_id: test_instance_id,
				migrations_path: "C:/migrations",
			}),
			http: { endpoint: new URL("http://127.0.0.1:4848/") },
		} as unknown as ForgeTransportBindingInput;
		const request = {
			headers: { cookie: `artisan_forge_session=${session_token}`, host: "artisan.example" },
		};
		expect(
			await Effect.runPromise(ForgeSessionAllowed(request as IncomingMessage, input)),
		).toBe(true);
		expect(
			ForgeOriginAllowed(
				{
					...request,
					headers: { ...request.headers, origin: "https://artisan.example" },
				} as IncomingMessage,
				input,
			),
		).toBe(true);
		expect(
			ForgeOriginAllowed(
				{
					...request,
					headers: { ...request.headers, origin: "https://other.example" },
				} as IncomingMessage,
				input,
			),
		).toBe(false);
		await authority_runtime.dispose();
	});
});
