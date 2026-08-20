import { generateKeyPairSync } from "node:crypto";
import {
	createReadStream,
	existsSync,
	mkdirSync,
	readFileSync,
	readdirSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { basename, join, resolve } from "node:path";

import { Checklist } from "@artisanstreet/checklist";
import {
	NodeChildProcessSpawner,
	NodeFileSystem,
	NodePath,
	NodeRuntime,
} from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";

import { module_build_steps } from "./build-modules.ts";

/**
 * One-command local release loop: build the distribution for this platform,
 * sign it with the machine-local development key, install it into the managed
 * Artisan root through ae-installer, then open the installed editor. The
 * counterpart of `pnpm run dev` for the packaged product.
 */

const repository_root = resolve(import.meta.dirname, "..", "..");
const release_root = resolve(repository_root, ".dist", "distribution-release");
const dev_root = resolve(repository_root, ".dist", "dev");
const signing_key_path = resolve(dev_root, "release-signing-key.pem");
const public_key_path = resolve(dev_root, "release-public-key.hex");
const installer_executable = resolve(repository_root, "target", "release", "ae-installer.exe");
const signing_key_id = "local-dev";

const release_version = Checklist.value<string>("release_version");
const release_public_key = Checklist.value<string>("release_public_key");
const release_origin = Checklist.value<string>("release_origin");

const parse_version = (value: string): [number, number, number] | undefined => {
	const match = /^(\d+)\.(\d+)\.(\d+)$/u.exec(value);

	if (match === null) return undefined;

	return [Number(match[1]), Number(match[2]), Number(match[3])];
};

const compare_versions = (left: [number, number, number], right: [number, number, number]) =>
	left[0] - right[0] || left[1] - right[1] || left[2] - right[2];

const detect_install_root = () => {
	const override = process.env.ARTISAN_INSTALL_ROOT ?? process.env.ARTISAN_HOME;

	if (override !== undefined && override.length > 0) return override;

	const local_app_data = process.env.LOCALAPPDATA;

	if (local_app_data === undefined || local_app_data.length === 0) {
		throw new Error("LOCALAPPDATA is not set; cannot locate the Artisan install root");
	}

	return join(local_app_data, "Artisan");
};

/**
 * The installer re-activates (and skips setup for) any version whose
 * directory already exists, so a fresh build must pick a version above every
 * version present on disk.
 */
const next_release_version = (install_root: string): string => {
	const candidates: Array<[number, number, number]> = [];
	const versions_root = join(install_root, "versions");

	if (existsSync(versions_root)) {
		for (const entry of readdirSync(versions_root)) {
			const parsed = parse_version(entry);
			if (parsed !== undefined) candidates.push(parsed);
		}
	}

	const installation_path = join(install_root, "installation.json");

	if (existsSync(installation_path)) {
		const installation = JSON.parse(readFileSync(installation_path, "utf8")) as {
			active_version?: string;
		};
		const parsed = parse_version(installation.active_version ?? "");
		if (parsed !== undefined) candidates.push(parsed);
	}

	if (candidates.length === 0) {
		const workspace = JSON.parse(
			readFileSync(resolve(repository_root, "package.json"), "utf8"),
		) as { version: string };

		return workspace.version;
	}

	const [major, minor, patch] = candidates.reduce((highest, candidate) =>
		compare_versions(candidate, highest) > 0 ? candidate : highest,
	);

	return `${major}.${minor}.${patch + 1}`;
};

/** Reuses the machine-local Ed25519 pair; generates one on first run. */
const ensure_signing_key = (): string => {
	if (existsSync(signing_key_path) && existsSync(public_key_path)) {
		return readFileSync(public_key_path, "utf8").trim();
	}

	mkdirSync(dev_root, { recursive: true });
	const pair = generateKeyPairSync("ed25519");
	const public_key_hex = pair.publicKey.export({ format: "der", type: "spki" }).toString("hex");

	writeFileSync(signing_key_path, pair.privateKey.export({ format: "pem", type: "pkcs8" }));
	writeFileSync(public_key_path, `${public_key_hex}\n`);

	return public_key_hex;
};

/**
 * Serves the release directory on a loopback ephemeral port for the duration
 * of the install. Loopback is the one plain-HTTP source ae-installer trusts,
 * and the artifact must resolve as a URL sibling of the manifest.
 */
const serve_release = () =>
	new Promise<{ close: () => void; origin: string }>((resolve_server, reject) => {
		const server = createServer((request, response) => {
			const file = join(
				release_root,
				basename(decodeURIComponent((request.url ?? "/").split("?")[0] ?? "/")),
			);

			if (!existsSync(file) || !statSync(file).isFile()) {
				response.writeHead(404);
				response.end();
				return;
			}

			response.writeHead(200, { "content-length": statSync(file).size });
			createReadStream(file).pipe(response);
		});

		server.once("error", reject);
		server.listen(0, "127.0.0.1", () => {
			const address = server.address();

			if (address === null || typeof address === "string") {
				reject(new Error("release server did not expose a port"));
				return;
			}

			resolve_server({
				close: () => server.close(),
				origin: `http://127.0.0.1:${address.port}`,
			});
		});
	});

/**
 * Bound to the run's scope rather than a try/finally, so the port is released
 * on interruption too — a Ctrl-C during install used to leave it listening.
 */
const ServeRelease = Effect.acquireRelease(
	Effect.tryPromise({ catch: (cause: unknown) => cause, try: serve_release }),
	(server) => Effect.sync(server.close),
).pipe(Effect.map((server) => server.origin));

const BuildProgram = Checklist.make(
	[
		{
			name: "prepare",
			provides: release_version,
			run: (step) => {
				const install_root = detect_install_root();
				const next = next_release_version(install_root);

				step.log(`platform  windows-x64`);
				step.log(`root      ${install_root}`);
				step.detail(`${next} → windows-x64`);

				return next;
			},
		},
		{
			name: "signing key",
			provides: release_public_key,
			run: () => ensure_signing_key(),
		},
		/**
		 * Ahead of every application bundle: the packaged editor resolves
		 * `@artisan/*` to these artifacts, so a stale `.dist` would ship silently.
		 */
		{ concurrency: 4, name: "modules", steps: module_build_steps() },
		{ name: "native build", run: "pnpm run build:native" },
		{
			env: (step) => {
				const version = step.get(release_version);

				return { ARTISAN_RELEASE_VERSION: version };
			},
			name: "desktop package",
			run: "pnpm run package:desktop",
		},
		{
			name: "verify installer",
			run: () => {
				if (!existsSync(installer_executable)) {
					throw new Error(`release ae-installer is missing at ${installer_executable}`);
				}
			},
		},
		{
			name: "clean release root",
			run: () => rmSync(release_root, { force: true, recursive: true }),
		},
		{
			cwd: repository_root,
			env: (step) => ({
				ARTISAN_RELEASE_SIGNING_KEY_FILE: signing_key_path,
				ARTISAN_RELEASE_SIGNING_KEY_ID: signing_key_id,
				ARTISAN_RELEASE_VERSION: step.get(release_version),
			}),
			name: "release artifact",
			run: [process.execPath, ".scripts/package/build-distribution-release-entry.ts"],
		},
		{ name: "release server", provides: release_origin, run: () => ServeRelease },
		{
			name: "install",
			run: (step) =>
				Checklist.command([
					installer_executable,
					"--manifest-url",
					`${step.get(release_origin)}/release-manifest.json`,
					"--public-key",
					step.get(release_public_key),
					/**
					 * Unattended, and with the installer free to retire whatever is
					 * still running a superseded version — otherwise Electron's
					 * single-instance lock hands the freshly built release straight
					 * back to the old window. A Forge that will not stop still fails
					 * the build rather than losing its work; that needs --force.
					 */
					"--yes",
				]),
		},
		{
			name: "open editor",
			run: () => Checklist.command([join(detect_install_root(), "bin", "ae.exe"), "open"]),
		},
	],
	{ subtitle: "windows-x64", title: "artisan build" },
);

const NodeProcessLive = NodeChildProcessSpawner.layer.pipe(
	Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
);

const Main = Effect.gen(function* () {
	if (process.platform !== "win32") {
		return yield* Effect.fail(
			new Error(
				`the ${process.platform} distribution is not implemented yet; only win32 builds exist`,
			),
		);
	}

	yield* BuildProgram;
});

NodeRuntime.runMain(Main.pipe(Effect.provide(NodeProcessLive)));
