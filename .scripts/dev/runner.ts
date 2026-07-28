/**
 * The universal development runner: one terminal runs the whole stack.
 *
 * `pnpm dev` supervises three lanes — the Forge watch-build, the Forge process
 * itself (launched from the staged bundle with runner-owned environment), and
 * the Vite frontend dev server — restarting the Forge whenever the watch-build
 * emits a new bundle. Ports derive from the worktree so parallel checkouts
 * never collide, and the runner owns the pairing secret so the Vite dev
 * server can mint same-origin pairing codes for the browser.
 */

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { randomBytes } from "node:crypto";
import {
	createWriteStream,
	existsSync,
	mkdirSync,
	readFileSync,
	statSync,
	watchFile,
	writeFileSync,
} from "node:fs";
import { createInterface } from "node:readline";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

export type RunnerMode = "dev" | "doctor" | "forge" | "pair" | "web";

const base_forge_port = 4848;
const base_web_port = 4849;
const max_instance_offset = 3000;

export const parse_runner_mode = (value: string | undefined): RunnerMode | undefined => {
	if (value === undefined || value === "dev") return "dev";
	if (value === "doctor" || value === "forge" || value === "pair" || value === "web")
		return value;
	return undefined;
};

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
	readonly frontend_static: string;
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
		frontend_static: join(repository_root, ".dist", "frontend"),
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
): Readonly<Record<string, string>> => ({
	ARTISAN_AUTH_TOKEN: auth_token,
	ARTISAN_DATABASE_PATH: join(paths.data_root, "artisan.sqlite"),
	ARTISAN_FORGE_LOG_PATH: join(paths.forge_home, "forge.log"),
	ARTISAN_FORGE_STATE_PATH: join(paths.forge_home, "state.json"),
	ARTISAN_HOME: paths.forge_home,
	ARTISAN_LISTEN_PORT: String(instance.forge_port),
	ARTISAN_MIGRATIONS_PATH: join(paths.forge_runtime, "migrations"),
	ARTISAN_NATIVE_RUNTIME: join(paths.forge_runtime, "native-runtime"),
	ARTISAN_STATIC_FRONTEND_ROOT: join(paths.forge_runtime, "frontend"),
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
			serve_frontend: true,
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
	readonly name: string;
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
	const mode = parse_runner_mode(process.argv[2]);
	if (mode === undefined) {
		console.error(`Unknown development mode: ${process.argv[2]}`);
		console.error("Usage: pnpm dev [dev|forge|web|pair|doctor]");
		process.exit(1);
	}

	const lanes: Lane[] = [];
	let shutting_down = false;

	const log = (line: string) => console.log(`[dev] ${line}`);

	const attach_lane = (name: string, child: ChildProcess): Lane => {
		register_shutdown();
		mkdirSync(paths.logs_root, { recursive: true });
		const file = createWriteStream(join(paths.logs_root, `${name}.log`), { flags: "a" });
		for (const stream of [child.stdout, child.stderr]) {
			if (stream === null) continue;
			createInterface({ input: stream }).on("line", (line) => {
				console.log(`[${name}] ${line}`);
				file.write(`${line}\n`);
			});
		}
		child.on("exit", (code) => {
			file.end();
			if (!shutting_down && code !== 0 && code !== null) {
				log(`${name} exited with code ${code}`);
			}
		});
		const lane = { child, name };
		lanes.push(lane);
		return lane;
	};

	const kill_tree = (child: ChildProcess) => {
		if (child.pid === undefined || child.exitCode !== null) return;
		if (process.platform === "win32") {
			spawnSync("taskkill", ["/pid", String(child.pid), "/T", "/F"], { stdio: "ignore" });
		} else {
			child.kill("SIGTERM");
		}
	};

	const shutdown = () => {
		if (shutting_down) return;
		shutting_down = true;
		if (lanes.length === 0) return;
		log("shutting down");
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
		name: string,
		pnpm_arguments: ReadonlyArray<string>,
		extra_environment: Record<string, string> = {},
	) =>
		attach_lane(
			name,
			spawn(`pnpm ${pnpm_arguments.join(" ")}`, {
				cwd: repository_root,
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

	const ensure_frontend_static = async () => {
		if (existsSync(paths.frontend_static)) return;
		log("building the static frontend once (required by the Forge watch-build)");
		const build = spawnSync("pnpm", ["--filter", "@artisan/frontend", "run", "build"], {
			cwd: repository_root,
			shell: true,
			stdio: "inherit",
		});
		if (build.status !== 0) {
			console.error("[dev] the initial frontend build failed");
			process.exit(1);
		}
	};

	let forge_lane: Lane | undefined;
	const start_forge = () => {
		const child = spawn(process.execPath, [paths.forge_bundle], {
			cwd: repository_root,
			env: { ...process.env, ...make_forge_environment(paths, instance, auth_token) },
			stdio: ["ignore", "pipe", "pipe", "ipc"],
			windowsHide: true,
		});
		forge_lane = attach_lane("forge", child);
		void (async () => {
			for (let attempt = 0; attempt < 120; attempt += 1) {
				if (child.exitCode !== null) return;
				if (await probe(`${instance.forge_origin}/health`)) {
					log(`forge ready at ${instance.forge_origin}`);
					return;
				}
				await sleep(500);
			}
			log(`forge did not answer on ${instance.forge_origin}/health`);
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
			/* Already disconnected. */
		}
		for (let waited = 0; waited < 5_000 && lane.child.exitCode === null; waited += 100) {
			await sleep(100);
		}
		kill_tree(lane.child);
	};

	const start_forge_when_bundled = async () => {
		while (!existsSync(paths.forge_bundle)) {
			if (shutting_down) return;
			await sleep(500);
		}
		start_forge();
		let restarting = false;
		watchFile(paths.forge_bundle, { interval: 1_000 }, () => {
			if (shutting_down || restarting) return;
			restarting = true;
			void (async () => {
				log("forge bundle changed; restarting the Forge");
				await stop_forge();
				/** A failed rebuild may leave no bundle; wait for the next good one. */
				while (!existsSync(paths.forge_bundle) && !shutting_down) {
					await sleep(500);
				}
				if (!shutting_down) start_forge();
				restarting = false;
			})();
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
			return `${instance.web_origin}/#pair=${encodeURIComponent(body.code)}`;
		} catch {
			return undefined;
		}
	};

	const doctor = async () => {
		const checks: Array<readonly [name: string, passed: boolean, detail: string]> = [];
		checks.push([
			"node",
			Number(process.versions.node.split(".")[0]) >= 22,
			`v${process.versions.node}`,
		]);
		checks.push([
			"frontend static build",
			existsSync(paths.frontend_static),
			paths.frontend_static,
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

		log(`instance offset ${instance.offset}`);
		log(`forge    ${instance.forge_origin}`);
		log(`frontend ${instance.web_origin}  (self-pairing enabled)`);

		if (mode === "dev" || mode === "forge") {
			await ensure_frontend_static();
			spawn_pnpm(
				"build",
				["exec", "vite", "build", "--watch", "--config", "forge.vite.config.ts"],
				{ ARTISAN_FORGE_WATCH: "1" },
			);
			void start_forge_when_bundled();
		}
		if (mode === "dev" || mode === "web") {
			spawn_pnpm("web", ["--filter", "@artisan/frontend", "run", "dev"], {
				ARTISAN_DEV_AUTH_TOKEN: auth_token,
				ARTISAN_FORGE_DEV_ORIGIN: instance.forge_origin,
				ARTISAN_FRONTEND_DEV_PORT: String(instance.web_port),
			});
		}
	};

	void main();
}
