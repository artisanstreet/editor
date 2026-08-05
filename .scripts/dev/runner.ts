/**
 * The universal development runner: one terminal runs the whole stack.
 *
 * `pnpm dev` supervises Artisan Forge's Rolldown watcher and clean daemon
 * restarts alongside the SvelteKit/Vite frontend dev server. Ports derive from
 * the worktree so parallel checkouts never collide, and the runner owns the
 * pairing secret so the frontend can mint same-origin pairing codes.
 */

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
	createWriteStream,
	existsSync,
	mkdirSync,
	readFileSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { createInterface } from "node:readline";
import { createRequire } from "node:module";
import { createConnection } from "node:net";
import { delimiter, dirname, join, resolve } from "node:path";
import type { Readable, Writable } from "node:stream";
import { fileURLToPath, pathToFileURL } from "node:url";

import { Schema } from "effect";
import { watch, type RolldownWatcher, type RolldownWatcherEvent } from "rolldown";

import { CreateForgeRolldownConfig } from "../../forge.rolldown.config.ts";
import {
	sanitize_dev_log_line,
	type DevEndpoint,
	type DevLaneDefinition,
	type DevLaneId,
	type DevLaneStatus,
	type DevTuiEvent,
} from "@artisan/dev-tui/model";

export type RunnerMode = "dev" | "doctor" | "forge" | "pair" | "web";

type ProcessLaneId = "forge" | "web";

const base_forge_port = 4848;
const base_web_port = 4849;
const max_instance_offset = 3000;
const max_dashboard_pending_events = 1_000;
const dependency_require = createRequire(import.meta.url);

const windows_openssl_installations = [
	join("C:", "Program Files", "OpenSSL-Win64"),
	join("C:", "Program Files (x86)", "OpenSSL-Win32"),
	join("C:", "Program Files", "OpenSSL"),
] as const;

/**
 * Winget's OpenSSL installer may leave its executable outside the inherited
 * PATH. Portless invokes `openssl` by name, so make the standard Windows
 * installation discoverable for the child process without mutating the shell.
 */
export const make_portless_environment = (
	environment: Readonly<Record<string, string | undefined>>,
): Readonly<Record<string, string>> => {
	const inherited_path = environment.Path ?? environment.PATH ?? "";
	if (process.platform !== "win32") {
		return Object.fromEntries(
			Object.entries(environment).filter(
				(entry): entry is [string, string] => entry[1] !== undefined,
			),
		);
	}

	const installation = windows_openssl_installations.find((root) =>
		existsSync(join(root, "bin", "openssl.exe")),
	);
	if (installation === undefined) {
		return Object.fromEntries(
			Object.entries(environment).filter(
				(entry): entry is [string, string] => entry[1] !== undefined,
			),
		);
	}

	const openssl_bin = join(installation, "bin");
	const openssl_config = [
		join(openssl_bin, "openssl.cnf"),
		join(openssl_bin, "openssl.cfg"),
		join(installation, "openssl.cnf"),
	].find((candidate) => existsSync(candidate));
	const path_key = environment.Path !== undefined ? "Path" : "PATH";
	return {
		...Object.fromEntries(
			Object.entries(environment).filter(
				(entry): entry is [string, string] => entry[1] !== undefined,
			),
		),
		[path_key]: [openssl_bin, inherited_path]
			.filter((value) => value.length > 0)
			.join(delimiter),
		...(environment.OPENSSL_CONF === undefined && openssl_config !== undefined
			? { OPENSSL_CONF: openssl_config }
			: {}),
	};
};

const resolve_bun_executable = (): string => {
	const package_path = dependency_require.resolve("bun/package.json");
	const manifest = JSON.parse(readFileSync(package_path, "utf8")) as {
		readonly bin: { readonly bun: string };
	};

	return join(dirname(package_path), manifest.bin.bun);
};

export const resolve_portless_entry = (): string =>
	join(dirname(fileURLToPath(import.meta.resolve("portless"))), "cli.js");

export const resolve_dev_tui_entry = (): string =>
	dependency_require.resolve("@artisan/dev-tui/entry");

export const enqueue_dev_tui_event = (
	pending_events: DevTuiEvent[],
	event: DevTuiEvent,
	max_pending_events = max_dashboard_pending_events,
): void => {
	const capacity = Number.isFinite(max_pending_events)
		? Math.max(1, Math.floor(max_pending_events))
		: max_dashboard_pending_events;

	while (pending_events.length >= capacity) {
		const stale_log_index = pending_events.findIndex((pending) => pending.type === "log");

		if (stale_log_index >= 0) {
			pending_events.splice(stale_log_index, 1);
			continue;
		}

		if (event.type === "log") return;

		pending_events.shift();
	}

	pending_events.push(event);
};

export const parse_runner_mode = (value: string | undefined): RunnerMode | undefined => {
	if (value === undefined || value === "dev") return "dev";
	if (value === "doctor" || value === "forge" || value === "pair" || value === "web")
		return value;
	return undefined;
};

export const make_dev_lane_definitions = (mode: RunnerMode): DevLaneDefinition[] => [
	{ id: "runner", label: "Overview", status: "ready" },
	...(mode === "dev" || mode === "web"
		? ([
				{ id: "web", label: "Artisan Editor", status: "waiting" },
			] satisfies DevLaneDefinition[])
		: []),
	...(mode === "dev" || mode === "forge"
		? ([
				{ id: "forge", label: "Artisan Forge", status: "waiting" },
			] satisfies DevLaneDefinition[])
		: []),
];

export const should_use_dev_tui = (input: {
	readonly environment: Readonly<Record<string, string | undefined>>;
	readonly stdin_is_tty: boolean | undefined;
	readonly stdout_is_tty: boolean | undefined;
}): boolean =>
	input.stdin_is_tty === true &&
	input.stdout_is_tty === true &&
	input.environment.ARTISAN_DEV_TUI !== "0" &&
	input.environment.CI === undefined &&
	input.environment.TERM !== "dumb";

/** Stable per-path offset so parallel worktrees get distinct, repeatable ports. */
export const hash_instance_offset = (path: string): number => {
	let hash = 2_166_136_261;
	for (const character of path.toLowerCase()) {
		hash ^= character.codePointAt(0) ?? 0;
		hash = Math.imul(hash, 16_777_619) >>> 0;
	}
	return (hash % max_instance_offset) + 1;
};

export interface DevInstance {
	readonly forge_origin: string;
	readonly forge_port: number;
	readonly offset: number;
	readonly web_origin: string;
	readonly web_port: number;
}

export interface DevSurfaceEndpoint {
	/** Exact static route name passed to `portless alias`. */
	readonly alias_name: string;
	readonly hostname: string;
	readonly origin: string;
}

export interface DevSurfaceEndpoints {
	readonly forge: DevSurfaceEndpoint;
	readonly web: DevSurfaceEndpoint;
}

export interface PortlessRouteRecord {
	readonly hostname: string;
	readonly kind: "alias" | "process";
	readonly port: number;
}

export const should_use_portless = (
	environment: Readonly<Record<string, string | undefined>>,
): boolean =>
	environment.PORTLESS !== "0" &&
	environment.PORTLESS !== "false" &&
	environment.PORTLESS !== "skip";

export const required_dev_node_major = (portless_enabled: boolean): number =>
	portless_enabled ? 24 : 22;

/** Explicit instances in one checkout need unique aliases in addition to unique ports. */
export const make_portless_base_names = (
	offset: number,
	has_explicit_instance: boolean,
): Readonly<{ forge: string; web: string }> => ({
	forge: has_explicit_instance ? `forge-${offset}` : "forge",
	web: has_explicit_instance ? `editor-${offset}` : "editor",
});

/** Decodes the worktree-aware URL returned by `portless get`. */
export const parse_portless_endpoint = (base_name: string, output: string): DevSurfaceEndpoint => {
	const url = Schema.decodeUnknownSync(Schema.URLFromString)(output.trim());
	if (url.protocol !== "https:" || !url.hostname.endsWith(".localhost")) {
		throw new Error(`Portless returned an unexpected local URL: ${url.toString()}`);
	}
	const alias_name = url.hostname.slice(0, -".localhost".length);
	if (alias_name !== base_name && !alias_name.endsWith(`.${base_name}`)) {
		throw new Error(`Portless returned ${url.hostname} for the ${base_name} route`);
	}

	return { alias_name, hostname: url.hostname, origin: url.origin };
};

/** Parses the stable route rows from `portless list`; headings stay ignored. */
export const parse_portless_route_listing = (output: string): PortlessRouteRecord[] =>
	output.split(/\r?\n/u).flatMap((line): PortlessRouteRecord[] => {
		const match = /^\s*(https?:\/\/\S+)\s+->\s+localhost:(\d+)\s+\((alias|pid \d+)\)\s*$/u.exec(
			line,
		);
		if (match === null) return [];
		const url = Schema.decodeUnknownSync(Schema.URLFromString)(match[1]);
		const port = Number(match[2]);
		if (!Number.isInteger(port) || port < 1 || port > 65_535) return [];
		return [
			{
				hostname: url.hostname,
				kind: match[3] === "alias" ? "alias" : "process",
				port,
			},
		];
	});

export type PortlessAliasPlan = "conflict" | "register" | "replace";

/** A dead static alias is recoverable only when it targets this exact instance port. */
export const plan_portless_alias = (
	existing: PortlessRouteRecord | undefined,
	expected_port: number,
	target_is_listening: boolean,
): PortlessAliasPlan => {
	if (existing === undefined) return target_is_listening ? "conflict" : "register";
	if (existing.kind !== "alias" || existing.port !== expected_port || target_is_listening) {
		return "conflict";
	}
	return "replace";
};

export const make_direct_dev_endpoints = (instance: DevInstance): DevSurfaceEndpoints => ({
	forge: {
		alias_name: "forge",
		hostname: "127.0.0.1",
		origin: instance.forge_origin,
	},
	web: {
		alias_name: "editor",
		hostname: "127.0.0.1",
		origin: instance.web_origin,
	},
});

/**
 * The main worktree keeps the well-known 4848/4849 pair; linked worktrees and
 * explicit instances derive an offset. Explicit port env vars win outright so
 * existing workflows keep behaving.
 */
export const derive_dev_instance = (input: {
	readonly environment: Readonly<Record<string, string | undefined>>;
	readonly is_main_worktree: boolean;
	readonly repository_root: string;
}): DevInstance => {
	const environment_instance = input.environment.ARTISAN_DEV_INSTANCE;
	const offset =
		environment_instance !== undefined && /^\d+$/.test(environment_instance)
			? (Number(environment_instance) % max_instance_offset) + 1
			: environment_instance !== undefined
				? hash_instance_offset(environment_instance)
				: input.is_main_worktree
					? 0
					: hash_instance_offset(input.repository_root);
	const forge_port = Number(
		input.environment.ARTISAN_FORGE_DEV_PORT ?? base_forge_port + offset * 2,
	);
	const web_port = Number(
		input.environment.ARTISAN_FRONTEND_DEV_PORT ?? base_web_port + offset * 2,
	);
	return {
		forge_origin: `http://127.0.0.1:${forge_port}`,
		forge_port,
		offset,
		web_origin: `http://127.0.0.1:${web_port}`,
		web_port,
	};
};

export interface DevPaths {
	readonly data_root: string;
	readonly forge_bundle: string;
	readonly forge_home: string;
	readonly forge_runtime: string;
	readonly logs_root: string;
	readonly repository_root: string;
	readonly secrets: string;
}

/**
 * Offset zero keeps the historical `.dist/dev` home; explicit instances get
 * isolated homes so two Forges in one worktree never share a database.
 */
export const derive_dev_paths = (repository_root: string, offset = 0): DevPaths => {
	const development =
		offset === 0
			? join(repository_root, ".dist", "dev")
			: join(repository_root, ".dist", "dev", `instance-${offset}`);
	const forge_runtime = join(repository_root, ".dist", "forge");
	return {
		data_root: join(development, "browser-forge"),
		forge_bundle: join(forge_runtime, "host.js"),
		forge_home: join(development, "forge-home"),
		forge_runtime,
		logs_root: join(development, "logs"),
		repository_root,
		secrets: join(development, "forge-home", "secrets.json"),
	};
};

/**
 * The Forge is configured entirely through its environment; this mirrors what
 * the installed CLI assembles, pointed at the repo's development home.
 */
export const make_forge_environment = (
	paths: DevPaths,
	instance: DevInstance,
	auth_token: string,
	endpoints: DevSurfaceEndpoints,
): Readonly<Record<string, string>> => ({
	ARTISAN_ALLOWED_HOSTNAMES: endpoints.forge.hostname,
	ARTISAN_ALLOWED_ORIGINS: endpoints.web.origin,
	ARTISAN_AUTH_TOKEN: auth_token,
	ARTISAN_DATABASE_PATH: join(paths.data_root, "artisan.sqlite"),
	/** Stated outright: the marker is no longer inferred from static hosting. */
	ARTISAN_FORGE_DEVELOPMENT: "1",
	ARTISAN_FORGE_LOG_PATH: join(paths.forge_home, "forge.log"),
	ARTISAN_FORGE_STATE_PATH: join(paths.forge_home, "state.json"),
	ARTISAN_HOME: paths.forge_home,
	ARTISAN_LISTEN_PORT: String(instance.forge_port),
	ARTISAN_MIGRATIONS_PATH: join(paths.forge_runtime, "migrations"),
	ARTISAN_NATIVE_RUNTIME: join(paths.forge_runtime, "native-runtime"),
	/**
	 * Engine adapters spawn through the staged process host; without this the
	 * host path resolves relative to the bundled chunk and does not exist.
	 * The system Node that runs the Forge also runs the host, so no staged
	 * executable override is needed.
	 */
	ARTISAN_WINDOWS_PROCESS_HOST: join(paths.forge_runtime, "windows-process-host.js"),
	NODE_PATH: join(paths.forge_runtime, "native-runtime"),
});

/** Matches the installed CLI's secrets.json so `ae open` interoperates. */
export const ensure_dev_secrets = (paths: DevPaths): string => {
	mkdirSync(paths.forge_home, { recursive: true });
	if (existsSync(paths.secrets)) {
		const decoded = JSON.parse(readFileSync(paths.secrets, "utf8")) as {
			readonly auth_token?: unknown;
		};
		if (typeof decoded.auth_token === "string" && decoded.auth_token.length >= 32) {
			return decoded.auth_token;
		}
	}
	const auth_token = randomBytes(32).toString("base64url");
	writeFileSync(paths.secrets, `${JSON.stringify({ auth_token, version: 1 })}\n`);
	return auth_token;
};

/** Matches the installed CLI's config.json so `ae open`/`ae status` interoperate. */
export const write_dev_config = (paths: DevPaths, instance: DevInstance): void => {
	writeFileSync(
		join(paths.forge_home, "config.json"),
		`${JSON.stringify({
			data_root: paths.data_root,
			listen_host: "127.0.0.1",
			listen_port: instance.forge_port,
			mode: "local",
			/**
			 * The development Forge serves the API and the WebSocket only. Vite
			 * serves the frontend, so hosting a second, separately built copy on
			 * the Forge origin would only offer a stale UI on a port where
			 * same-origin self-pairing does not exist.
			 */
			serve_frontend: false,
			version: 1,
		})}\n`,
	);
};

const is_main_worktree = (repository_root: string): boolean => {
	try {
		return statSync(join(repository_root, ".git")).isDirectory();
	} catch {
		return true;
	}
};

const probe = async (url: string): Promise<boolean> => {
	try {
		const controller = new AbortController();
		const timeout = setTimeout(() => controller.abort(), 2_000);
		const response = await fetch(url, {
			cache: "no-store",
			/** No keep-alive: a lingering socket handle stalls one-shot modes. */
			headers: { connection: "close" },
			signal: controller.signal,
		});
		clearTimeout(timeout);
		await response.arrayBuffer();
		return response.ok;
	} catch {
		return false;
	}
};

const sleep = (milliseconds: number) =>
	new Promise<void>((resolve_sleep) => setTimeout(resolve_sleep, milliseconds));

interface Lane {
	readonly child: ChildProcess;
	readonly name: ProcessLaneId;
}

interface DevDashboard {
	readonly close: () => void;
	readonly send: (event: DevTuiEvent) => boolean;
}

const script_entry = process.argv[1];
const runner_is_entry =
	script_entry !== undefined && import.meta.url === pathToFileURL(resolve(script_entry)).href;

if (runner_is_entry) {
	const repository_root = resolve(import.meta.dirname, "..", "..");
	const instance = derive_dev_instance({
		environment: process.env,
		is_main_worktree: is_main_worktree(repository_root),
		repository_root,
	});
	const paths = derive_dev_paths(repository_root, instance.offset);
	let endpoints = make_direct_dev_endpoints(instance);
	const mode = parse_runner_mode(process.argv[2]);
	if (mode === undefined) {
		console.error(`Unknown development mode: ${process.argv[2]}`);
		console.error("Usage: pnpm dev [dev|forge|web|pair|doctor]");
		process.exit(1);
	}

	const lanes: Lane[] = [];
	let shutting_down = false;
	let dashboard: DevDashboard | undefined;

	const send_dashboard_event = (event: DevTuiEvent): boolean => dashboard?.send(event) ?? false;

	const log = (line: string, lane_id: DevLaneId = "runner") => {
		const sanitized_line = sanitize_dev_log_line(line);

		if (send_dashboard_event({ lane_id, line: sanitized_line, type: "log" })) return;

		console.log(`[dev] ${line}`);
	};

	const set_lane_status = (lane_id: DevLaneId, status: DevLaneStatus) => {
		send_dashboard_event({ lane_id, status, type: "status" });
	};

	const portless_enabled = should_use_portless(process.env);
	const portless_environment = make_portless_environment({
		...process.env,
		/** Artisan's development contract is the stable HTTPS `.localhost` pair. */
		NO_COLOR: "1",
		PORTLESS_HTTPS: "1",
		PORTLESS_LAN: "0",
		PORTLESS_TLD: "localhost",
	});
	let portless_entry: string | undefined;
	const registered_portless_aliases: Array<{
		readonly endpoint: DevSurfaceEndpoint;
		readonly port: number;
	}> = [];
	const node_major = Number(process.versions.node.split(".")[0]);
	const assert_dev_node_version = (): void => {
		const required_major = required_dev_node_major(portless_enabled);
		if (node_major >= required_major) return;
		throw new Error(
			portless_enabled
				? `Portless requires Node ${required_major} or newer; upgrade Node or set PORTLESS=0`
				: `Artisan development requires Node ${required_major} or newer`,
		);
	};
	const run_portless = (arguments_: ReadonlyArray<string>, capture_output = false): string => {
		portless_entry ??= resolve_portless_entry();
		const result = spawnSync(process.execPath, [portless_entry, ...arguments_], {
			cwd: repository_root,
			encoding: "utf8",
			env: portless_environment,
			stdio: capture_output ? ["ignore", "pipe", "pipe"] : "inherit",
			windowsHide: true,
		});
		if (result.error !== undefined) throw result.error;
		if (result.status !== 0) {
			const detail = capture_output ? result.stderr.trim() : "";
			throw new Error(
				`portless ${arguments_.join(" ")} failed${detail.length > 0 ? `: ${detail}` : ""}`,
			);
		}
		return capture_output ? result.stdout.trim() : "";
	};

	const resolve_portless_endpoints = (): DevSurfaceEndpoints => {
		const names = make_portless_base_names(
			instance.offset,
			process.env.ARTISAN_DEV_INSTANCE !== undefined,
		);
		return {
			forge: parse_portless_endpoint(names.forge, run_portless(["get", names.forge], true)),
			web: parse_portless_endpoint(names.web, run_portless(["get", names.web], true)),
		};
	};

	const read_portless_routes = (): PortlessRouteRecord[] =>
		parse_portless_route_listing(run_portless(["list"], true));

	const loopback_port_is_listening = (port: number): Promise<boolean> =>
		new Promise((accept) => {
			const socket = createConnection({ host: "127.0.0.1", port });
			let settled = false;
			const settle = (listening: boolean) => {
				if (settled) return;
				settled = true;
				socket.destroy();
				accept(listening);
			};
			socket.setTimeout(500, () => settle(false));
			socket.once("connect", () => settle(true));
			socket.once("error", () => settle(false));
		});

	const register_portless_alias = async (
		endpoint: DevSurfaceEndpoint,
		port: number,
	): Promise<void> => {
		const existing = read_portless_routes().find(
			(route) => route.hostname === endpoint.hostname,
		);
		const target_is_listening = await loopback_port_is_listening(port);
		const plan = plan_portless_alias(existing, port, target_is_listening);
		if (plan === "conflict") {
			throw new Error(
				existing === undefined
					? `${endpoint.origin} cannot be registered because 127.0.0.1:${port} is already in use`
					: `${endpoint.origin} is already registered to port ${existing.port} or its target is running`,
			);
		}
		if (plan === "replace") {
			run_portless(["alias", "--remove", endpoint.alias_name]);
		}
		run_portless(["alias", endpoint.alias_name, String(port)]);
		registered_portless_aliases.push({ endpoint, port });
	};

	const unregister_portless_aliases = (): void => {
		for (const registration of registered_portless_aliases.splice(0).reverse()) {
			const { endpoint, port } = registration;
			try {
				const current = read_portless_routes().find(
					(route) => route.hostname === endpoint.hostname,
				);
				if (current === undefined) continue;
				if (current.kind !== "alias" || current.port !== port) {
					console.error(
						`[dev] leaving replacement Portless route ${endpoint.origin} untouched`,
					);
					continue;
				}
				run_portless(["alias", "--remove", endpoint.alias_name]);
			} catch (cause) {
				console.error(
					`[dev] could not remove Portless alias ${endpoint.alias_name}: ${cause instanceof Error ? cause.message : String(cause)}`,
				);
			}
		}
	};

	const setup_portless = async (): Promise<void> => {
		if (!portless_enabled) return;
		assert_dev_node_version();
		run_portless(["proxy", "start"]);
		endpoints = resolve_portless_endpoints();
		try {
			if (mode === "dev" || mode === "forge") {
				await register_portless_alias(endpoints.forge, instance.forge_port);
			}
			if (mode === "dev" || mode === "web") {
				await register_portless_alias(endpoints.web, instance.web_port);
			}
		} catch (cause) {
			unregister_portless_aliases();
			throw cause;
		}
	};

	const attach_lane = (name: ProcessLaneId, child: ChildProcess): Lane => {
		register_shutdown();
		mkdirSync(paths.logs_root, { recursive: true });
		const file = createWriteStream(join(paths.logs_root, `${name}.log`), { flags: "a" });

		set_lane_status(name, "starting");
		child.once("spawn", () => set_lane_status(name, "running"));
		child.once("error", () => set_lane_status(name, "failed"));

		for (const stream of [child.stdout, child.stderr]) {
			if (stream === null) continue;
			createInterface({ input: stream }).on("line", (line) => {
				const sanitized_line = sanitize_dev_log_line(line);

				if (!send_dashboard_event({ lane_id: name, line: sanitized_line, type: "log" })) {
					console.log(`[${name}] ${line}`);
				}
				if (name === "web" && /Local:\s+http/u.test(line)) {
					set_lane_status(name, "ready");
				}
				file.write(`${sanitized_line}\n`);
			});
		}
		child.on("exit", (code) => {
			file.end();
			set_lane_status(name, shutting_down || code === 0 ? "stopped" : "failed");
			if (!shutting_down && code !== 0 && code !== null) {
				log(`${name} exited with code ${code}`, name);
			}
		});
		const lane = { child, name };
		lanes.push(lane);
		return lane;
	};

	const start_dashboard = (lane_definitions: ReadonlyArray<DevLaneDefinition>) => {
		let bun_executable: string;
		let dashboard_entry: string;
		let child: ChildProcess;

		try {
			bun_executable = resolve_bun_executable();
			dashboard_entry = resolve_dev_tui_entry();
			child = spawn(bun_executable, [dashboard_entry], {
				cwd: repository_root,
				env: process.env,
				stdio: ["inherit", "inherit", "inherit", "pipe", "pipe"],
				windowsHide: false,
			});
		} catch (cause) {
			console.error(
				`[dev] dashboard could not start: ${cause instanceof Error ? cause.message : String(cause)}`,
			);

			return undefined;
		}
		const event_stream = child.stdio[3] as Writable | null;
		const command_stream = child.stdio[4] as Readable | null;
		let active = event_stream !== null && command_stream !== null;
		let backpressured = false;
		let closing = false;
		let force_close_timeout: NodeJS.Timeout | undefined;
		const pending_events: DevTuiEvent[] = [];

		if (!active || event_stream === null || command_stream === null) {
			child.kill();
			return undefined;
		}

		const write_event = (event: DevTuiEvent): boolean =>
			event_stream.write(`${JSON.stringify(event)}\n`);

		const flush_pending_events = (): void => {
			backpressured = false;

			try {
				while (active && pending_events.length > 0) {
					const event = pending_events.shift();

					if (event === undefined) return;
					if (write_event(event)) continue;

					backpressured = true;
					return;
				}
			} catch {
				active = false;
			}
		};

		const send = (event: DevTuiEvent): boolean => {
			if (!active || event_stream.destroyed || event_stream.writableEnded) return false;

			if (backpressured) {
				enqueue_dev_tui_event(pending_events, event);
				return true;
			}

			try {
				backpressured = !write_event(event);
				return true;
			} catch {
				active = false;
				return false;
			}
		};

		createInterface({ input: command_stream }).on("line", (line) => {
			try {
				const command = JSON.parse(line) as { readonly type?: unknown };

				if (command.type === "shutdown") shutdown();
			} catch {
				/** A malformed display command is ignored; the supervisor keeps ownership. */
			}
		});
		event_stream.once("error", () => {
			active = false;
			pending_events.length = 0;
		});
		event_stream.on("drain", flush_pending_events);
		child.once("error", (cause) => {
			active = false;
			pending_events.length = 0;
			console.error(`[dev] dashboard could not start: ${cause.message}`);
		});
		child.once("exit", (code) => {
			active = false;
			pending_events.length = 0;
			if (force_close_timeout !== undefined) clearTimeout(force_close_timeout);
			event_stream.destroy();
			command_stream.destroy();
			if (!closing && !shutting_down) {
				console.error(
					`[dev] dashboard exited with code ${code ?? "unknown"}; using plain logs`,
				);
			}
		});

		const control: DevDashboard = {
			close: () => {
				if (closing) return;
				closing = true;
				active = false;
				pending_events.length = 0;

				if (!event_stream.destroyed && !event_stream.writableEnded) {
					event_stream.end(`${JSON.stringify({ type: "shutdown" })}\n`);
				}

				force_close_timeout = setTimeout(() => {
					event_stream.destroy();
					command_stream.destroy();
					if (child.pid !== undefined && child.exitCode === null) child.kill();
				}, 2_000);
				force_close_timeout.unref();
			},
			send,
		};

		const dashboard_endpoints: ReadonlyArray<DevEndpoint> = [
			{ label: `Artisan Editor ${endpoints.web.origin}`, url: endpoints.web.origin },
			{ label: `Artisan Forge ${endpoints.forge.origin}`, url: endpoints.forge.origin },
		];

		control.send({
			endpoints: dashboard_endpoints,
			lanes: lane_definitions,
			title: `Artisan dev · instance ${instance.offset}`,
			type: "configure",
		});

		return control;
	};

	const kill_tree = (child: ChildProcess) => {
		if (child.pid === undefined || child.exitCode !== null) return;

		if (process.platform === "win32") {
			spawnSync("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore" });
			return;
		}

		try {
			process.kill(-child.pid, "SIGTERM");
		} catch {
			child.kill("SIGTERM");
		}
	};

	const shutdown = () => {
		if (shutting_down) return;
		log("shutting down");
		shutting_down = true;
		dashboard?.close();
		unregister_portless_aliases();
		const watcher = forge_watcher;
		forge_watcher = undefined;
		if (watcher !== undefined) void watcher.close().catch(() => undefined);
		if (lanes.length === 0) return;
		for (const lane of lanes) kill_tree(lane.child);
	};
	/** Registered lazily: query modes must exit without teardown side effects. */
	let shutdown_registered = false;
	const register_shutdown = () => {
		if (shutdown_registered) return;
		shutdown_registered = true;
		process.once("SIGINT", shutdown);
		process.once("SIGTERM", shutdown);
		process.once("exit", shutdown);
	};

	/** Arguments are static runner-owned strings, joined once for the shell. */
	const spawn_pnpm = (
		name: ProcessLaneId,
		pnpm_arguments: ReadonlyArray<string>,
		extra_environment: Record<string, string> = {},
	) =>
		attach_lane(
			name,
			spawn(`pnpm ${pnpm_arguments.join(" ")}`, {
				cwd: repository_root,
				detached: process.platform !== "win32",
				env: { ...process.env, ...extra_environment },
				shell: true,
				stdio: ["ignore", "pipe", "pipe"],
				windowsHide: true,
			}),
		);

	/** Doctor stays read-only; every run mode prepares the home first. */
	let auth_token = "";
	const prepare_dev_home = () => {
		auth_token = ensure_dev_secrets(paths);
		write_dev_config(paths, instance);
		mkdirSync(paths.data_root, { recursive: true });
	};

	let forge_lane: Lane | undefined;
	let forge_watcher: RolldownWatcher | undefined;
	let forge_build_generation = 0;
	let applied_forge_build_generation = 0;
	let restarting_forge = false;
	let forge_build_finalization_failed = false;
	let forge_bundle_finalization = Promise.resolve();
	const start_forge = () => {
		const child = spawn(process.execPath, [paths.forge_bundle], {
			cwd: repository_root,
			detached: process.platform !== "win32",
			env: {
				...process.env,
				...make_forge_environment(paths, instance, auth_token, endpoints),
			},
			stdio: ["ignore", "pipe", "pipe", "ipc"],
			windowsHide: true,
		});
		forge_lane = attach_lane("forge", child);
		void (async () => {
			for (let attempt = 0; attempt < 120; attempt += 1) {
				if (child.exitCode !== null) return;
				if (await probe(`${instance.forge_origin}/health`)) {
					set_lane_status("forge", "ready");
					log(`forge ready at ${endpoints.forge.origin}`, "forge");
					return;
				}
				await sleep(500);
			}
			set_lane_status("forge", "failed");
			log(`forge did not answer on ${instance.forge_origin}/health`, "forge");
		})();
	};

	/** The Forge honors parent disconnect as a shutdown request; force-kill is the fallback. */
	const stop_forge = async () => {
		const lane = forge_lane;
		forge_lane = undefined;
		if (lane === undefined || lane.child.exitCode !== null) return;
		try {
			lane.child.disconnect();
		} catch {
			/** Already disconnected. */
		}
		for (let waited = 0; waited < 5_000 && lane.child.exitCode === null; waited += 100) {
			await sleep(100);
		}
		kill_tree(lane.child);
	};

	const restart_forge_after_build = async () => {
		if (
			shutting_down ||
			restarting_forge ||
			forge_build_generation === applied_forge_build_generation
		)
			return;

		restarting_forge = true;
		try {
			if (!existsSync(paths.forge_bundle)) {
				set_lane_status("forge", "failed");
				log(`Forge bundle is missing: ${paths.forge_bundle}`, "forge");
				return;
			}

			set_lane_status("forge", "starting");
			if (forge_lane !== undefined) {
				log("Forge bundle changed; restarting Artisan Forge", "forge");
				await stop_forge();
			}
			if (!shutting_down) {
				start_forge();
				applied_forge_build_generation = forge_build_generation;
			}
		} finally {
			restarting_forge = false;
			if (!shutting_down && forge_build_generation !== applied_forge_build_generation)
				void restart_forge_after_build();
		}
	};

	const start_forge_watcher = () => {
		try {
			forge_watcher = watch(CreateForgeRolldownConfig({ watch: true }));
		} catch (cause) {
			set_lane_status("forge", "failed");
			log(
				`Forge watcher could not start: ${cause instanceof Error ? cause.message : String(cause)}`,
				"forge",
			);
			return;
		}

		forge_watcher.on("event", async (event: RolldownWatcherEvent) => {
			if (event.code === "START") {
				forge_build_finalization_failed = false;
				log("building Artisan Forge", "forge");
				return;
			}

			if (event.code === "BUNDLE_END") {
				forge_bundle_finalization = event.result.close().then(
					() => log(`Forge bundle ready in ${event.duration}ms`, "forge"),
					(cause: unknown) => {
						forge_build_finalization_failed = true;
						set_lane_status("forge", "failed");
						log(
							`Forge bundle finalization failed: ${cause instanceof Error ? cause.message : String(cause)}`,
							"forge",
						);
					},
				);
				await forge_bundle_finalization;
				return;
			}

			if (event.code === "ERROR") {
				forge_build_finalization_failed = true;
				set_lane_status("forge", "failed");
				log(
					`Forge bundle failed: ${event.error instanceof Error ? event.error.message : String(event.error)}`,
					"forge",
				);
				try {
					await event.result.close();
				} catch {
					/** Preserve the original build error. */
				}
				return;
			}

			if (event.code === "END") {
				await forge_bundle_finalization;
				if (forge_build_finalization_failed) return;
				forge_build_generation += 1;
				void restart_forge_after_build();
			}
		});
	};

	const mint_pair_url = async (): Promise<string | undefined> => {
		try {
			const minted = await fetch(`${instance.forge_origin}/api/pair/request`, {
				headers: { authorization: `Bearer ${auth_token}`, connection: "close" },
				method: "POST",
			});
			if (!minted.ok) return undefined;
			const body = (await minted.json()) as { readonly code?: unknown };
			if (typeof body.code !== "string" || body.code.length === 0) return undefined;
			return `${endpoints.web.origin}/#pair=${encodeURIComponent(body.code)}`;
		} catch {
			return undefined;
		}
	};

	/**
	 * Opens the Vite origin with a pairing fragment once both servers answer.
	 *
	 * In-page self-pairing only runs after the gate exhausts its retries, so a
	 * cold start would otherwise sit on the remedy screen for the length of the
	 * retry budget. Opening a pre-paired tab also settles which origin is the
	 * development UI: the Forge origin serves no frontend at all.
	 */
	const open_paired_browser = async () => {
		for (let attempt = 0; attempt < 120 && !shutting_down; attempt += 1) {
			if (
				(await probe(`${instance.forge_origin}/health`)) &&
				(await probe(instance.web_origin))
			) {
				const url = await mint_pair_url();
				if (url === undefined) break;
				set_lane_status("web", "ready");
				log(`opening ${endpoints.web.origin} (paired)`, "web");
				if (process.platform === "win32") {
					spawnSync("cmd", ["/c", "start", "", url], { stdio: "ignore" });
				}
				return;
			}
			await sleep(1_000);
		}
		if (!shutting_down) {
			log(
				`could not open a paired browser; open ${endpoints.web.origin} and it self-pairs`,
				"web",
			);
		}
	};

	const doctor = async () => {
		const checks: Array<readonly [name: string, passed: boolean, detail: string]> = [];
		checks.push([
			"node",
			node_major >= required_dev_node_major(portless_enabled),
			`v${process.versions.node} (requires ${required_dev_node_major(portless_enabled)}+)`,
		]);
		checks.push(["forge bundle", existsSync(paths.forge_bundle), paths.forge_bundle]);
		checks.push(["secrets", existsSync(paths.secrets), paths.secrets]);
		const forge_healthy = await probe(`${instance.forge_origin}/health`);
		checks.push(["forge health", forge_healthy, `${instance.forge_origin}/health`]);
		const web_healthy = await probe(instance.web_origin);
		checks.push(["web dev server", web_healthy, instance.web_origin]);
		const state_path = join(paths.forge_home, "state.json");
		if (existsSync(state_path)) {
			const state = JSON.parse(readFileSync(state_path, "utf8")) as {
				readonly pid?: number;
			};
			let alive = false;
			if (typeof state.pid === "number") {
				try {
					process.kill(state.pid, 0);
					alive = true;
				} catch {
					alive = false;
				}
			}
			checks.push([
				"forge state pid",
				alive || !forge_healthy,
				`pid ${state.pid ?? "unknown"} ${alive ? "alive" : "not running (stale state.json)"}`,
			]);
		}
		for (const [name, passed, detail] of checks) {
			console.log(`${passed ? "PASS" : "FAIL"}  ${name.padEnd(22)} ${detail}`);
		}
		/** Let the loop drain naturally: process.exit here races undici sockets on Windows. */
		process.exitCode = checks.every(([, passed]) => passed) ? 0 : 1;
	};

	const main = async () => {
		if (mode === "doctor") {
			await doctor();
			return;
		}
		prepare_dev_home();
		if (mode === "pair") {
			if (portless_enabled) {
				try {
					assert_dev_node_version();
					endpoints = resolve_portless_endpoints();
				} catch (cause) {
					console.error(
						`[dev] could not resolve Portless routes: ${cause instanceof Error ? cause.message : String(cause)}`,
					);
					process.exitCode = 1;
					return;
				}
			}
			const url = await mint_pair_url();
			if (url === undefined) {
				console.error(
					`[dev] could not mint a pairing code from ${instance.forge_origin}; is the Forge running?`,
				);
				process.exit(1);
			}
			console.log(url);
			if (process.platform === "win32") {
				spawnSync("cmd", ["/c", "start", "", url], { stdio: "ignore" });
			}
			return;
		}

		register_shutdown();
		try {
			await setup_portless();
		} catch (cause) {
			console.error(
				`[dev] Portless setup failed: ${cause instanceof Error ? cause.message : String(cause)}`,
			);
			shutdown();
			process.exitCode = 1;
			return;
		}

		if (
			should_use_dev_tui({
				environment: process.env,
				stdin_is_tty: process.stdin.isTTY,
				stdout_is_tty: process.stdout.isTTY,
			})
		) {
			dashboard = start_dashboard(make_dev_lane_definitions(mode));
		}

		log(`instance offset ${instance.offset}`);
		log(`forge    ${endpoints.forge.origin}`);
		log(`frontend ${endpoints.web.origin}  (self-pairing enabled)`);

		if (mode === "dev" || mode === "forge") {
			start_forge_watcher();
		}
		if (mode === "dev" || mode === "web") {
			spawn_pnpm("web", ["--filter", "@artisan/frontend", "run", "dev"], {
				ARTISAN_DEV_AUTH_TOKEN: auth_token,
				ARTISAN_FORGE_DEV_ORIGIN: instance.forge_origin,
				ARTISAN_FRONTEND_DEV_PORT: String(instance.web_port),
				ARTISAN_FRONTEND_PUBLIC_HOSTNAME: endpoints.web.hostname,
			});
			void open_paired_browser();
		}
	};

	void main();
}
