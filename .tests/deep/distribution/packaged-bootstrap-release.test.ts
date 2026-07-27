import { execFile } from "node:child_process";
import { access, cp, mkdtemp, readFile, readdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve, win32 } from "node:path";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

import { afterEach, describe, expect, it } from "vitest";

const ExecFile = promisify(execFile);
const temporary_roots: Array<string> = [];
const registry_roots: Array<string> = [];
const running_installations: Array<{
	readonly environment: NodeJS.ProcessEnv;
	readonly permanent_ae: string;
}> = [];

interface PackagedManifest {
	readonly artifacts: ReadonlyArray<{ readonly file_name: string }>;
	readonly product_version: string;
}

const Exists = (path: string) =>
	access(path).then(
		() => true,
		() => false,
	);

const Run = (
	executable: string,
	argv: ReadonlyArray<string>,
	environment: NodeJS.ProcessEnv,
	timeout = 90_000,
) =>
	ExecFile(executable, [...argv], {
		env: environment,
		maxBuffer: 4 * 1024 * 1024,
		timeout,
		windowsHide: true,
	});

const QuoteCmdArgument = (value: string) =>
	`"${value.replaceAll("%", "%%").replaceAll('"', '""')}"`;

const RunCmd = (
	path: string,
	argv: ReadonlyArray<string>,
	environment: NodeJS.ProcessEnv,
	timeout = 90_000,
) =>
	ExecFile(
		process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe",
		[
			"/d",
			"/s",
			"/c",
			`""${path}"${argv.length === 0 ? "" : ` ${argv.map(QuoteCmdArgument).join(" ")}`}"`,
		],
		{
			env: environment,
			maxBuffer: 4 * 1024 * 1024,
			timeout,
			windowsHide: true,
			windowsVerbatimArguments: true,
		},
	);

const WaitUntil = async (predicate: () => Promise<boolean>, message: string, timeout = 30_000) => {
	const deadline = Date.now() + timeout;
	while (!(await predicate())) {
		if (Date.now() >= deadline) throw new Error(message);
		await new Promise((resolve_wait) => setTimeout(resolve_wait, 100));
	}
};

const QueryRegistry = (key: string, value_name?: string) =>
	Run(
		"reg.exe",
		["query", key, ...(value_name === undefined ? ["/ve"] : ["/v", value_name])],
		process.env,
	);

afterEach(async () => {
	await Promise.all(
		running_installations.splice(0).map(async ({ environment, permanent_ae }) => {
			if (await Exists(permanent_ae))
				await RunCmd(permanent_ae, ["stop"], environment).catch(() => undefined);
		}),
	);
	await Promise.all(
		registry_roots
			.splice(0)
			.map((root) =>
				Run("reg.exe", ["delete", root, "/f"], process.env).catch(() => undefined),
			),
	);
	await Promise.all(
		temporary_roots.splice(0).map((root) => rm(root, { force: true, recursive: true })),
	);
});

describe("packed npm bootstrap with signed Windows release", () => {
	it.skipIf(process.env.ARTISAN_PACKAGED_BOOTSTRAP_GATE !== "1")(
		"installs from an absent home through the actual packed ae.cmd",
		async () => {
			if (process.platform !== "win32")
				throw new Error("The Windows packaged bootstrap gate requires Windows");

			const tarball = process.env.ARTISAN_BOOTSTRAP_TARBALL;
			if (tarball === undefined) throw new Error("ARTISAN_BOOTSTRAP_TARBALL is required");
			const release_root = resolve(
				process.env.ARTISAN_DISTRIBUTION_RELEASE_ROOT ?? ".dist/distribution-release",
			);
			const manifest = JSON.parse(
				await readFile(join(release_root, "release-manifest.json"), "utf8"),
			) as PackagedManifest;
			for (const path of [
				join(release_root, "release-manifest.sig"),
				...manifest.artifacts.map((artifact) => join(release_root, artifact.file_name)),
			])
				expect(await Exists(path)).toBe(true);

			const root = await mkdtemp(join(tmpdir(), "artisan-packaged-acceptance-"));
			temporary_roots.push(root);
			const app_data = join(root, "AppData", "Roaming");
			const local_app_data = join(root, "AppData", "Local");
			const user_profile = join(root, "User");
			const installation_root = join(local_app_data, "Artisan");
			const npm_prefix = join(root, "npm");
			const fetch_log = join(root, "release-fetch.log");
			const preload_path = join(root, "local-release-fetch-preload.ts");
			await cp(
				resolve(".tests/fixtures/distribution/local-release-fetch-preload.ts"),
				preload_path,
			);

			const namespace = `gate_${process.pid}_${Date.now()}`;
			const registry_root = `HKCU\\Software\\ArtisanAcceptance\\${namespace}`;
			registry_roots.push(registry_root);

			const npm_location = (await Run("where.exe", ["npm.cmd"], process.env)).stdout
				.split(/\r?\n/u)
				.map((line) => line.trim())
				.find((line) => win32.isAbsolute(line));
			if (npm_location === undefined) throw new Error("npm.cmd is unavailable");
			const npm_cli = join(
				win32.dirname(process.execPath),
				"node_modules",
				"npm",
				"bin",
				"npm-cli.js",
			);
			await Run(
				process.execPath,
				[npm_cli, "install", "--global", "--prefix", npm_prefix, resolve(tarball)],
				process.env,
			);
			const bootstrap_ae = join(npm_prefix, "ae.cmd");
			expect(await Exists(bootstrap_ae)).toBe(true);
			expect(await Exists(installation_root)).toBe(false);

			const environment: NodeJS.ProcessEnv = {
				...process.env,
				APPDATA: app_data,
				ARTISAN_ACCEPTANCE_FETCH_LOG: fetch_log,
				ARTISAN_ACCEPTANCE_REGISTRY_NAMESPACE: namespace,
				ARTISAN_ACCEPTANCE_RELEASE_ROOT: release_root,
				ARTISAN_NPM_EXECUTABLE: npm_location,
				ARTISAN_NPM_PREFIX: npm_prefix,
				LOCALAPPDATA: local_app_data,
				NODE_OPTIONS: `--import=${pathToFileURL(preload_path).href}`,
				USERPROFILE: user_profile,
			};
			for (const name of [
				"ARTISAN_HOME",
				"ARTISAN_RELEASE_OWNER",
				"ARTISAN_RELEASE_REPOSITORY",
				"ARTISAN_RELEASE_KEY_ID",
				"ARTISAN_RELEASE_PUBLIC_KEY_BASE64",
				"ARTISAN_RELEASE_PUBLIC_KEY_FILE",
				"npm_config_prefix",
			])
				delete environment[name];

			// This is the documented first-use contract: the disposable command is
			// invoked with no subcommand, finalizes Forge, then delegates plain `ae`
			// to the permanent client launcher.
			const permanent_ae = join(installation_root, "bin", "ae.cmd");
			running_installations.push({ environment, permanent_ae });
			await RunCmd(bootstrap_ae, [], environment, 120_000).catch(async (cause: unknown) => {
				const forge_log = await readFile(
					join(installation_root, "profiles", "default", "forge.log"),
					"utf8",
				).catch(() => "<Forge log unavailable>");
				const forge_state = await readFile(
					join(installation_root, "profiles", "default", "state.json"),
					"utf8",
				).catch(() => "<Forge state unavailable>");
				const requests = await readFile(fetch_log, "utf8").catch(
					() => "<Fetch log unavailable>",
				);
				throw new Error(
					`Packaged bootstrap failed.\nForge state:\n${forge_state}\nForge log:\n${forge_log}\nRequests:\n${requests}`,
					{
						cause,
					},
				);
			});
			await WaitUntil(
				() => Exists(bootstrap_ae).then((exists) => !exists),
				"Disposable ae.cmd was not removed",
			);
			expect(await Exists(join(npm_prefix, "node_modules", "artisan-editor"))).toBe(false);
			expect(await Exists(permanent_ae)).toBe(true);
			expect(await readdir(join(installation_root, "profiles"))).toEqual(["default"]);
			expect(await readFile(join(installation_root, "current"), "utf8")).toContain(
				manifest.product_version,
			);

			const fetched = await readFile(fetch_log, "utf8");
			expect(fetched).toContain("/release-manifest.json");
			expect(fetched).toContain("/release-manifest.sig");
			expect(fetched).toContain(manifest.artifacts[0]?.file_name);

			const shortcut_root = join(
				app_data,
				"Microsoft",
				"Windows",
				"Start Menu",
				"Programs",
				"Artisan",
			);
			for (const shortcut of [
				"Artisan Editor.lnk",
				"Start Artisan Forge.lnk",
				"Artisan Forge Logs.lnk",
				"Uninstall Artisan.lnk",
			])
				expect(await Exists(join(shortcut_root, shortcut))).toBe(true);
			expect((await QueryRegistry(`${registry_root}\\Environment`, "Path")).stdout).toContain(
				join(installation_root, "bin"),
			);
			expect(
				(await QueryRegistry(`${registry_root}\\Classes\\artisan\\shell\\open\\command`))
					.stdout,
			).toContain(join(installation_root, "bin", "Artisan Editor.cmd"));

			const permanent_environment = Object.fromEntries(
				Object.entries(environment).filter(([name]) => name.toLowerCase() !== "path"),
			);
			permanent_environment.Path = `${join(installation_root, "bin")};${
				environment.Path ?? environment.PATH ?? ""
			}`;
			const resolved = await Run(
				process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe",
				["/d", "/c", "where ae"],
				permanent_environment,
			);
			expect(
				resolved.stdout
					.split(/\r?\n/u)
					.map((line) => line.trim().toLowerCase())
					.filter(Boolean)[0],
			).toBe(permanent_ae.toLowerCase());
			expect(
				(await RunCmd(permanent_ae, ["--version"], permanent_environment)).stdout,
			).toMatch(/0\.1\.0/u);
			expect(
				JSON.parse(
					(await RunCmd(permanent_ae, ["status", "--json"], permanent_environment))
						.stdout,
				),
			).toEqual({ state: "running" });
			const doctor = JSON.parse(
				(await RunCmd(permanent_ae, ["doctor", "--json"], permanent_environment)).stdout,
			) as { readonly distribution: { readonly healthy: boolean } };
			expect(doctor.distribution.healthy).toBe(true);
			const update = await RunCmd(permanent_ae, ["update"], permanent_environment).catch(
				(cause: unknown) => {
					const failure = cause as { readonly stderr?: string; readonly stdout?: string };
					throw new Error(
						`Permanent update failed.\nstdout:\n${failure.stdout ?? ""}\nstderr:\n${
							failure.stderr ?? ""
						}`,
						{ cause },
					);
				},
			);
			expect(update.stdout).toContain("is current");
			await RunCmd(permanent_ae, ["doctor", "--fix", "--json"], permanent_environment).catch(
				(cause: unknown) => {
					const failure = cause as { readonly stderr?: string; readonly stdout?: string };
					throw new Error(
						`Permanent doctor repair failed.\nstdout:\n${failure.stdout ?? ""}\nstderr:\n${
							failure.stderr ?? ""
						}`,
						{ cause },
					);
				},
			);
			await RunCmd(permanent_ae, ["restart"], permanent_environment);
			await RunCmd(permanent_ae, ["stop"], permanent_environment);
			await RunCmd(permanent_ae, ["uninstall", "--remove-data"], permanent_environment).catch(
				(cause: unknown) => {
					const failure = cause as { readonly stderr?: string; readonly stdout?: string };
					throw new Error(
						`Permanent uninstall failed.\nstdout:\n${failure.stdout ?? ""}\nstderr:\n${
							failure.stderr ?? ""
						}`,
						{ cause },
					);
				},
			);

			await WaitUntil(
				() => Exists(installation_root).then((exists) => !exists),
				"Detached permanent uninstall did not remove Artisan",
			).catch(async (cause: unknown) => {
				const helpers = (await readdir(tmpdir()))
					.filter((name) => name.startsWith("artisan-uninstall-"))
					.sort();
				const helper_diagnostics = await Promise.all(
					helpers.map(async (name) => ({
						name,
						content: await readFile(join(tmpdir(), name), "utf8").catch(() => ""),
					})),
				);
				const processes = await Run(
					"powershell.exe",
					[
						"-NoLogo",
						"-NoProfile",
						"-NonInteractive",
						"-Command",
						`Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -like '${installation_root.replaceAll("'", "''")}*' } | Select-Object ProcessId,Name,ExecutablePath,CommandLine | ConvertTo-Json -Compress`,
					],
					process.env,
				).catch(() => ({ stdout: "<process inspection failed>" }));
				throw new Error(
					`Detached permanent uninstall did not remove Artisan.\nProcesses:\n${processes.stdout}\nHelpers:\n${JSON.stringify(
						helper_diagnostics,
					)}`,
					{ cause },
				);
			});
			for (const shortcut of [
				"Artisan Editor.lnk",
				"Start Artisan Forge.lnk",
				"Artisan Forge Logs.lnk",
				"Uninstall Artisan.lnk",
			])
				expect(await Exists(join(shortcut_root, shortcut))).toBe(false);
			const remaining_path = await QueryRegistry(
				`${registry_root}\\Environment`,
				"Path",
			).catch(() => undefined);
			expect(remaining_path?.stdout ?? "").not.toContain(join(installation_root, "bin"));
			await expect(QueryRegistry(`${registry_root}\\Classes\\artisan`)).rejects.toMatchObject(
				{ code: 1 },
			);
		},
		240_000,
	);
});
