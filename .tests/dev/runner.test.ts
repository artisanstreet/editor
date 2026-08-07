import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import { Effect } from "effect";

import {
	derive_dev_instance,
	derive_dev_paths,
	enqueue_dev_tui_event,
	ensure_dev_secrets,
	hash_instance_offset,
	make_forge_environment,
	make_dev_lane_definitions,
	make_artisan_runner_adapter_environment,
	make_artisan_runner_options,
	make_artisan_runner_process,
	make_dashboard_dev_endpoints,
	make_portless_environment,
	make_portless_base_names,
	parse_portless_endpoint,
	parse_portless_route_listing,
	parse_runner_mode,
	plan_portless_alias,
	required_dev_node_major,
	resolve_dev_tui_entry,
	resolve_portless_entry,
	should_use_dev_tui,
	should_wrap_artisan_runner,
	should_use_portless,
	route_artisan_runner_output,
	write_dev_config,
} from "../../.scripts/dev/runner";

import type { DevTuiEvent } from "@artisan/dev-tui/model";

const temporary_roots: string[] = [];
const make_root = () => {
	const root = mkdtempSync(join(tmpdir(), "artisan-dev-runner-"));
	temporary_roots.push(root);
	return root;
};

afterEach(() => {
	for (const root of temporary_roots.splice(0)) {
		rmSync(root, { force: true, recursive: true });
	}
});

describe("dev runner modes", () => {
	it("defaults to dev and accepts every documented mode", () => {
		expect(parse_runner_mode(undefined)).toBe("dev");
		expect(parse_runner_mode("dev")).toBe("dev");
		for (const mode of ["doctor", "forge", "pair", "web"]) {
			expect(parse_runner_mode(mode)).toBe(mode);
		}
		expect(parse_runner_mode("bogus")).toBeUndefined();
	});

	it("owns every public development entrypoint", () => {
		const manifest = JSON.parse(readFileSync("package.json", "utf8")) as {
			readonly devDependencies: Readonly<Record<string, string>>;
			readonly scripts: Readonly<Record<string, string>>;
		};

		expect(manifest.scripts.dev).toBe("node .scripts/dev/runner.ts");
		expect(manifest.scripts["dev:web"]).toBe("node .scripts/dev/runner.ts web");
		expect(manifest.scripts["dev:forge"]).toBe("node .scripts/dev/runner.ts forge");
		expect(manifest.scripts["dev:open"]).toBe("node .scripts/dev/runner.ts pair");
		expect(manifest.scripts["dev:pair"]).toBe("node .scripts/dev/runner.ts pair");
		expect(manifest.devDependencies.portless).toBe("0.15.5");
	});
});

describe("Effect runner adapter", () => {
	it("wraps only ordinary development modes and never recurses into its marked adapter", () => {
		for (const mode of ["dev", "forge", "web"] as const) {
			expect(should_wrap_artisan_runner({ environment: {}, mode })).toBe(true);
		}
		for (const mode of ["doctor", "pair"] as const) {
			expect(should_wrap_artisan_runner({ environment: {}, mode })).toBe(false);
		}
		expect(
			should_wrap_artisan_runner({
				environment: { ARTISAN_DEV_RUNNER_ADAPTER: "1" },
				mode: "dev",
			}),
		).toBe(false);
	});

	it("marks the adapter and disables only its legacy dashboard", () => {
		expect(
			make_artisan_runner_adapter_environment({
				ARTISAN_DEV_TUI: "1",
				CUSTOM: "retained",
			}),
		).toEqual({
			ARTISAN_DEV_RUNNER_ADAPTER: "1",
			ARTISAN_DEV_TUI: "0",
			CUSTOM: "retained",
		});
	});

	it("routes raw adapter lines to the public lanes and exposes readiness", async () => {
		await expect(
			Effect.runPromise(
				route_artisan_runner_output({
					line: "[web] \u001B[32mLocal: http://127.0.0.1:4849\u001B[0m",
					process_id: "runner",
					stream: "stdout",
				}),
			),
		).resolves.toEqual([
			{
				lane_id: "web",
				line: "\u001B[32mLocal: http://127.0.0.1:4849\u001B[0m",
				status: "ready",
			},
		]);
		await expect(
			Effect.runPromise(
				route_artisan_runner_output({
					line: "[forge] forge ready at https://forge.localhost",
					process_id: "runner",
					stream: "stderr",
				}),
			),
		).resolves.toEqual([
			{
				lane_id: "forge",
				line: "forge ready at https://forge.localhost",
				status: "ready",
			},
		]);
	});

	it("supplies exact Artisan lanes and honors the plain-log escape hatch", () => {
		const instance = {
			forge_origin: "http://127.0.0.1:4848",
			forge_port: 4848,
			offset: 0,
			web_origin: "http://127.0.0.1:4849",
			web_port: 4849,
		};
		const endpoints = {
			forge: {
				alias_name: "forge",
				hostname: "forge.localhost",
				origin: "https://forge.localhost",
			},
			web: {
				alias_name: "editor",
				hostname: "editor.localhost",
				origin: "https://editor.localhost",
			},
		};
		const options = make_artisan_runner_options("dev", instance, endpoints, {});
		expect(options.dashboard).toBe("auto");
		expect(options.lanes).toEqual([
			{ id: "runner", name: "Overview", status: "ready" },
			{ id: "web", name: "Artisan Editor", status: "waiting" },
			{ id: "forge", name: "Artisan Forge", status: "waiting" },
		]);
		expect(make_artisan_runner_process("runner.ts", "dev", {}).lane_ids).toEqual([
			"runner",
			"web",
			"forge",
		]);
		expect(make_artisan_runner_process("runner.ts", "web", {}).lane_ids).toEqual([
			"runner",
			"web",
		]);
		expect(make_artisan_runner_process("runner.ts", "forge", {}).lane_ids).toEqual([
			"runner",
			"forge",
		]);
		expect(
			make_artisan_runner_options("web", instance, endpoints, { ARTISAN_DEV_TUI: "0" })
				.dashboard,
		).toBe("never");
	});

	it("uses declarative HTTPS Portless endpoints for the outer dashboard", () => {
		const instance = {
			forge_origin: "http://127.0.0.1:4864",
			forge_port: 4864,
			offset: 8,
			web_origin: "http://127.0.0.1:4865",
			web_port: 4865,
		};
		expect(make_dashboard_dev_endpoints(instance, {})).toEqual({
			forge: {
				alias_name: "forge",
				hostname: "forge.localhost",
				origin: "https://forge.localhost",
			},
			web: {
				alias_name: "editor",
				hostname: "editor.localhost",
				origin: "https://editor.localhost",
			},
		});
		expect(make_dashboard_dev_endpoints(instance, { ARTISAN_DEV_INSTANCE: "8" })).toMatchObject(
			{
				forge: { origin: "https://forge-8.localhost" },
				web: { origin: "https://editor-8.localhost" },
			},
		);
		expect(make_dashboard_dev_endpoints(instance, { PORTLESS: "0" })).toMatchObject({
			forge: { origin: "http://127.0.0.1:4864" },
			web: { origin: "http://127.0.0.1:4865" },
		});
	});
});

describe("Portless development routes", () => {
	it("makes a standard Windows OpenSSL installation available to Portless", () => {
		const environment = make_portless_environment({ Path: "C:\\existing", TEST: "1" });
		const environment_path = environment.Path ?? environment.PATH ?? "";

		if (process.platform === "win32" && environment_path.includes("OpenSSL-Win64\\bin")) {
			expect(environment.OPENSSL_CONF).toMatch(/openssl\.(cfg|cnf)$/u);
			expect(environment_path.startsWith("C:\\Program Files\\OpenSSL-Win64\\bin")).toBe(true);
		} else {
			expect(environment).toEqual({ Path: "C:\\existing", TEST: "1" });
		}
	});

	it("decodes the canonical routes and preserves Portless's worktree prefix", () => {
		expect(parse_portless_endpoint("editor", "https://editor.localhost\n")).toEqual({
			alias_name: "editor",
			hostname: "editor.localhost",
			origin: "https://editor.localhost",
		});
		expect(parse_portless_endpoint("forge", "https://fix-tools.forge.localhost\n")).toEqual({
			alias_name: "fix-tools.forge",
			hostname: "fix-tools.forge.localhost",
			origin: "https://fix-tools.forge.localhost",
		});
	});

	it("rejects a route that does not match Artisan's HTTPS localhost contract", () => {
		expect(() => parse_portless_endpoint("editor", "http://editor.localhost")).toThrow();
		expect(() => parse_portless_endpoint("editor", "https://attacker.invalid")).toThrow();
		expect(() => parse_portless_endpoint("editor", "https://forge.localhost")).toThrow();
	});

	it("reserves distinct aliases for explicit instances and supports the Portless bypass", () => {
		expect(make_portless_base_names(0, false)).toEqual({ forge: "forge", web: "editor" });
		expect(make_portless_base_names(8, true)).toEqual({
			forge: "forge-8",
			web: "editor-8",
		});
		expect(should_use_portless({})).toBe(true);
		for (const value of ["0", "false", "skip"]) {
			expect(should_use_portless({ PORTLESS: value })).toBe(false);
		}
		expect(required_dev_node_major(true)).toBe(24);
		expect(required_dev_node_major(false)).toBe(22);
	});

	it("recovers only a stale static alias for the same physical port", () => {
		const routes = parse_portless_route_listing(`
Active routes:

  https://editor.localhost  ->  localhost:4849  (alias)
  https://docs.localhost  ->  localhost:5173  (pid 42)
`);
		expect(routes).toEqual([
			{ hostname: "editor.localhost", kind: "alias", port: 4849 },
			{ hostname: "docs.localhost", kind: "process", port: 5173 },
		]);
		expect(plan_portless_alias(undefined, 4849, false)).toBe("register");
		expect(plan_portless_alias(undefined, 4849, true)).toBe("conflict");
		expect(plan_portless_alias(routes[0], 4849, false)).toBe("replace");
		expect(plan_portless_alias(routes[0], 4849, true)).toBe("conflict");
		expect(plan_portless_alias(routes[0], 6001, false)).toBe("conflict");
		expect(plan_portless_alias(routes[1], 5173, false)).toBe("conflict");
	});

	it("resolves the pinned Portless CLI through its package manifest", () => {
		expect(resolve_portless_entry()).toMatch(/[\\/]portless[\\/]dist[\\/]cli\.js$/u);
	});
});

describe("dev dashboard capability", () => {
	it("keeps the dashboard process rail focused on the two product surfaces", () => {
		expect(make_dev_lane_definitions("dev").map((lane) => lane.label)).toEqual([
			"Overview",
			"Artisan Editor",
			"Artisan Forge",
		]);
	});

	it("uses the dashboard only in an interactive terminal", () => {
		expect(
			should_use_dev_tui({ environment: {}, stdin_is_tty: true, stdout_is_tty: true }),
		).toBe(true);
		expect(
			should_use_dev_tui({ environment: {}, stdin_is_tty: false, stdout_is_tty: true }),
		).toBe(false);
		expect(
			should_use_dev_tui({ environment: {}, stdin_is_tty: true, stdout_is_tty: false }),
		).toBe(false);
	});

	it("honors CI and the explicit plain-log escape hatch", () => {
		expect(
			should_use_dev_tui({
				environment: { CI: "1" },
				stdin_is_tty: true,
				stdout_is_tty: true,
			}),
		).toBe(false);
		expect(
			should_use_dev_tui({
				environment: { ARTISAN_DEV_TUI: "0" },
				stdin_is_tty: true,
				stdout_is_tty: true,
			}),
		).toBe(false);
		expect(
			should_use_dev_tui({
				environment: { TERM: "dumb" },
				stdin_is_tty: true,
				stdout_is_tty: true,
			}),
		).toBe(false);
	});

	it("resolves the Bun entry through the package export", () => {
		expect(resolve_dev_tui_entry()).toMatch(/[\\/]modules[\\/]dev-tui[\\/]src[\\/]entry\.ts$/u);
	});

	it("bounds backpressured events while retaining control updates", () => {
		const pending_events: DevTuiEvent[] = [
			{ lane_id: "api", line: "stale", type: "log" },
			{ lane_id: "api", status: "running", type: "status" },
		];

		enqueue_dev_tui_event(
			pending_events,
			{ lane_id: "api", status: "ready", type: "status" },
			2,
		);

		expect(pending_events).toEqual([
			{ lane_id: "api", status: "running", type: "status" },
			{ lane_id: "api", status: "ready", type: "status" },
		]);

		enqueue_dev_tui_event(
			pending_events,
			{ lane_id: "api", line: "discarded", type: "log" },
			2,
		);

		expect(pending_events).toHaveLength(2);
	});
});

describe("dev instance derivation", () => {
	it("keeps the well-known ports for the main worktree", () => {
		const instance = derive_dev_instance({
			environment: {},
			is_main_worktree: true,
			repository_root: "C:/repo",
		});

		expect(instance).toMatchObject({ forge_port: 4848, offset: 0, web_port: 4849 });
		expect(instance.forge_origin).toBe("http://127.0.0.1:4848");
		expect(instance.web_origin).toBe("http://127.0.0.1:4849");
	});

	it("derives a stable non-zero offset for linked worktrees", () => {
		const first = derive_dev_instance({
			environment: {},
			is_main_worktree: false,
			repository_root: "C:/worktrees/feature-a",
		});
		const second = derive_dev_instance({
			environment: {},
			is_main_worktree: false,
			repository_root: "C:/worktrees/feature-a",
		});
		const other = derive_dev_instance({
			environment: {},
			is_main_worktree: false,
			repository_root: "C:/worktrees/feature-b",
		});

		expect(first.offset).toBeGreaterThan(0);
		expect(first).toEqual(second);
		expect(first.forge_port).not.toBe(4848);
		expect(other.offset).not.toBe(first.offset);
		expect(first.web_port).toBe(first.forge_port + 1);
	});

	it("bounds hashed offsets inside the instance range", () => {
		for (const path of ["a", "b", "C:/x/y/z", "/very/long/worktree/path/indeed"]) {
			const offset = hash_instance_offset(path);
			expect(offset).toBeGreaterThanOrEqual(1);
			expect(offset).toBeLessThanOrEqual(3000);
		}
	});

	it("honors explicit instance and port environment overrides", () => {
		const numbered = derive_dev_instance({
			environment: { ARTISAN_DEV_INSTANCE: "2" },
			is_main_worktree: true,
			repository_root: "C:/repo",
		});
		expect(numbered.offset).toBe(3);
		expect(numbered.forge_port).toBe(4848 + 6);

		const explicit_ports = derive_dev_instance({
			environment: {
				ARTISAN_FORGE_DEV_PORT: "6000",
				ARTISAN_FRONTEND_DEV_PORT: "6001",
			},
			is_main_worktree: false,
			repository_root: "C:/worktrees/feature-a",
		});
		expect(explicit_ports.forge_port).toBe(6000);
		expect(explicit_ports.web_port).toBe(6001);
	});
});

describe("dev home interop", () => {
	it("writes CLI-compatible secrets once and reuses them across runs", () => {
		const paths = derive_dev_paths(make_root());
		const first = ensure_dev_secrets(paths);
		const second = ensure_dev_secrets(paths);

		expect(first).toBe(second);
		expect(first.length).toBeGreaterThanOrEqual(32);
		const stored = JSON.parse(readFileSync(paths.secrets, "utf8")) as Record<string, unknown>;
		expect(stored).toEqual({ auth_token: first, version: 1 });
	});

	it("writes a CLI-compatible config for the derived instance", () => {
		const paths = derive_dev_paths(make_root());
		ensure_dev_secrets(paths);
		write_dev_config(paths, {
			forge_origin: "http://127.0.0.1:4848",
			forge_port: 4848,
			offset: 0,
			web_origin: "http://127.0.0.1:4849",
			web_port: 4849,
		});

		const config = JSON.parse(
			readFileSync(join(paths.forge_home, "config.json"), "utf8"),
		) as Record<string, unknown>;
		expect(config).toEqual({
			data_root: paths.data_root,
			listen_host: "127.0.0.1",
			listen_port: 4848,
			mode: "local",
			serve_frontend: false,
			version: 1,
		});
	});

	it("isolates explicit instances under their own development home", () => {
		const shared = derive_dev_paths("C:/repo");
		const isolated = derive_dev_paths("C:/repo", 8);

		expect(shared.forge_home).toContain(join(".dist", "dev", "forge-home"));
		expect(isolated.forge_home).toContain(join(".dist", "dev", "instance-8", "forge-home"));
		expect(isolated.data_root).not.toBe(shared.data_root);
		/** The staged Forge runtime is a build artifact and stays shared. */
		expect(isolated.forge_runtime).toBe(shared.forge_runtime);
	});

	it("assembles the complete Forge environment from the staged runtime", () => {
		const paths = derive_dev_paths("C:/repo");
		const environment = make_forge_environment(
			paths,
			{
				forge_origin: "http://127.0.0.1:4848",
				forge_port: 4848,
				offset: 0,
				web_origin: "http://127.0.0.1:4849",
				web_port: 4849,
			},
			"token-token-token-token-token-token",
			{
				forge: {
					alias_name: "forge",
					hostname: "forge.localhost",
					origin: "https://forge.localhost",
				},
				web: {
					alias_name: "editor",
					hostname: "editor.localhost",
					origin: "https://editor.localhost",
				},
			},
		);

		expect(environment.ARTISAN_LISTEN_PORT).toBe("4848");
		expect(environment.ARTISAN_ALLOWED_HOSTNAMES).toBe("forge.localhost");
		expect(environment.ARTISAN_ALLOWED_ORIGINS).toBe("https://editor.localhost");
		expect(environment.ARTISAN_AUTH_TOKEN).toBe("token-token-token-token-token-token");
		expect(environment.ARTISAN_DATABASE_PATH).toContain("browser-forge");
		expect(environment.ARTISAN_MIGRATIONS_PATH).toContain("migrations");
		expect(environment.ARTISAN_NATIVE_RUNTIME).toContain("native-runtime");
		/** Vite owns the development UI, so the Forge is handed no frontend to serve. */
		expect(environment).not.toHaveProperty("ARTISAN_STATIC_FRONTEND_ROOT");
		expect(environment.ARTISAN_FORGE_DEVELOPMENT).toBe("1");
		expect(environment.ARTISAN_WINDOWS_PROCESS_HOST).toContain("windows-process-host.js");
		expect(environment.ARTISAN_HOME).toBe(paths.forge_home);
	});
});
