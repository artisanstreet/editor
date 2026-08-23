import { mkdtemp, readdir, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { extname, join, resolve } from "node:path";

import { Effect, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	decode_forge_config,
	ForgeControlAuthority,
	make_forge_control_authority_layer,
	start_forge_http,
} from "../../modules/forge/src/index";
import { ForgeChildEnvironment } from "../../modules/cli/src/node-launcher";

const test_instance_id = "9adf07cc-4e56-4be2-bb70-6a1f6f7b2b41";

const launcher_artifact = {
	broker_executable_path: "C:/artisan/Artisan Broker.exe",
	executable_path: "C:/artisan/Artisan Forge.exe",
	host_entry_path: "C:/artisan/host.js",
	migrations_path: "C:/artisan/migrations",
	native_runtime_path: "C:/artisan/native-runtime",
	node_executable_path: "C:/artisan/node.exe",
	static_frontend_root: "C:/artisan/frontend",
	windows_process_host_path: "C:/artisan/windows-process-host.js",
};

const launcher_input = (serve_frontend: boolean | undefined) => ({
	config: {
		data_root: "C:/artisan/data",
		listen_host: "127.0.0.1" as const,
		listen_port: 0,
		...(serve_frontend === undefined ? {} : { serve_frontend }),
	},
	instance_id: "forge_gate",
	token: "secret",
});

/**
 * Architecture gate: static web hosting is a development capability.
 * An installed home composes a Forge that serves only its health and
 * control/WS surfaces; the Electron editor renders the bundled frontend.
 */
describe("static hosting production gate", () => {
	const closers: Array<() => Promise<void>> = [];

	afterEach(async () => {
		await Promise.all(closers.splice(0).map((close) => close()));
	});

	it("keeps the installed composition free of a static frontend root", () => {
		const config = decode_forge_config({
			database_path: "C:/artisan/data.sqlite",
			instance_id: test_instance_id,
			migrations_path: "C:/artisan/migrations",
		});
		expect(config.static_frontend_root).toBeUndefined();

		const child_environment = (serve_frontend: boolean | undefined) =>
			ForgeChildEnvironment(
				launcher_input(serve_frontend),
				launcher_artifact,
				{ inherited: {}, node_path: undefined },
				{ log_path: "C:/artisan/forge.log", state_path: "C:/artisan/state.json" },
			);
		expect(child_environment(undefined)).not.toHaveProperty("ARTISAN_STATIC_FRONTEND_ROOT");
		expect(child_environment(false)).not.toHaveProperty("ARTISAN_STATIC_FRONTEND_ROOT");
		expect(child_environment(true)).toMatchObject({
			ARTISAN_BROKER_PATH: "C:/artisan/Artisan Broker.exe",
			ARTISAN_BROKER_REQUIRED: "1",
			ARTISAN_STATIC_FRONTEND_ROOT: "C:/artisan/frontend",
		});
	});

	it("serves no SPA route while health and pairing keep working with hosting off", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-hosting-gate-"));
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const host = await Effect.runPromise(
			start_forge_http(
				decode_forge_config({
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

		expect((await fetch(host.endpoint)).status).toBe(404);
		expect((await fetch(new URL("/t/workspace_1/12345", host.endpoint))).status).toBe(404);
		expect((await fetch(new URL("/index.html", host.endpoint))).status).toBe(404);

		const health = await fetch(new URL("/health", host.endpoint));
		/** A hosting-off Forge never reports itself as a development instance. */
		expect(await health.json()).toMatchObject({
			development: false,
			service: "artisan-forge",
			status: "ready",
		});

		/**
		 * The installed editor pairs cross-origin from `artisan://app`; the
		 * exchange must succeed on a hosting-off Forge with the exact origin
		 * echoed and a cross-site-capable session cookie.
		 */
		const preflight = await fetch(new URL("/api/pair", host.endpoint), {
			headers: {
				"access-control-request-headers": "content-type,traceparent",
				"access-control-request-method": "POST",
				origin: "artisan://app",
			},
			method: "OPTIONS",
		});
		expect(preflight.status).toBe(204);
		/** The typed HTTP client sends tracing headers; the origin allow-list is the boundary. */
		expect(preflight.headers.get("access-control-allow-headers")).toBe(
			"content-type,traceparent",
		);

		const code = await Effect.runPromise(authority.RequestPair);
		const paired = await fetch(new URL("/api/pair", host.endpoint), {
			body: JSON.stringify({ code }),
			headers: { "content-type": "application/json", origin: "artisan://app" },
			method: "POST",
		});
		expect(paired.status).toBe(200);
		expect(paired.headers.get("access-control-allow-origin")).toBe("artisan://app");
		expect(paired.headers.get("set-cookie")).toContain("SameSite=None");
	});

	/**
	 * The development marker and static hosting are independent: `pnpm dev`
	 * serves its UI from Vite and hands the Forge no frontend, yet the shell
	 * badge must still identify the instance.
	 */
	it("marks a development instance that serves no frontend", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-dev-marker-"));
		const authority_runtime = ManagedRuntime.make(make_forge_control_authority_layer());
		const authority = await authority_runtime.runPromise(ForgeControlAuthority);
		const host = await Effect.runPromise(
			start_forge_http(
				decode_forge_config({
					database_path: join(directory, "artisan.sqlite"),
					development: true,
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

		expect((await fetch(host.endpoint)).status).toBe(404);
		expect(await (await fetch(new URL("/health", host.endpoint))).json()).toMatchObject({
			development: true,
			status: "ready",
		});
	});

	it("keeps --serve-frontend an explicit development opt-in across every setup path", async () => {
		const scripts_root = resolve(import.meta.dirname, "../../.scripts");
		const script_files = (
			await readdir(scripts_root, { recursive: true, withFileTypes: true })
		).filter(
			(entry) =>
				entry.isFile() && [".ps1", ".ts", ".sh", ".cmd"].includes(extname(entry.name)),
		);
		const serving_scripts: string[] = [];
		for (const entry of script_files) {
			const path = join(entry.parentPath, entry.name);
			if ((await readFile(path, "utf8")).includes("--serve-frontend")) {
				serving_scripts.push(entry.name);
			}
		}
		/** Vite owns development hosting; repository scripts do not opt Forge into it. */
		expect(serving_scripts).toEqual([]);

		const rust_cli = await readFile(
			resolve(import.meta.dirname, "../../modules/cli/rust/commands.rs"),
			"utf8",
		);
		expect(rust_cli).toContain("serve_frontend: bool");
		expect(rust_cli).not.toContain("default_value_t = true");

		const rust_process = await readFile(
			resolve(import.meta.dirname, "../../modules/cli/rust/process.rs"),
			"utf8",
		);
		expect(rust_process).toContain("if config.serve_frontend {");

		const ts_entry = await readFile(
			resolve(import.meta.dirname, "../../modules/cli/src/entry.ts"),
			"utf8",
		);
		expect(ts_entry).toContain('Flag.boolean("serve-frontend")');
		expect(ts_entry).toContain("serve_frontend: input.serve_frontend");
	});
});
