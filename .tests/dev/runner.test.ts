import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import {
	derive_dev_instance,
	derive_dev_paths,
	ensure_dev_secrets,
	hash_instance_offset,
	make_forge_environment,
	parse_runner_mode,
	write_dev_config,
} from "../../.scripts/dev/runner";

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
		);

		expect(environment.ARTISAN_LISTEN_PORT).toBe("4848");
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
