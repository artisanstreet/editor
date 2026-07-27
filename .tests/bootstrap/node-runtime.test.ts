import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import {
	BootstrapInstallFailure,
	BootstrapInstaller,
	BootstrapInstallerUnavailableLive,
	make_node_bootstrap_layer,
	make_node_permanent_ae_layer_with_timeout,
	ParseDetachedCleanup,
	PermanentAe,
	ResolveInstallationRoot,
	ResolveNpmExecutable,
	ResolveNpmPrefix,
	type NpmCleanupPlan,
} from "../../modules/bootstrap/src";

describe("bootstrap Node runtime", () => {
	it("resolves the product root independently of a project", () => {
		expect(
			ResolveInstallationRoot({ LOCALAPPDATA: "C:\\Users\\test\\AppData\\Local" }, "win32"),
		).toBe("C:\\Users\\test\\AppData\\Local\\Artisan");
		expect(ResolveInstallationRoot({ HOME: "/home/test" }, "linux")).toBe(
			"/home/test/.local/share/artisan",
		);
	});

	it("uses an explicit npm executable override without PATH lookup", () => {
		expect(
			ResolveNpmExecutable({ ARTISAN_NPM_EXECUTABLE: "C:\\Tools\\npm.cmd" }, "win32"),
		).toBe("C:\\Tools\\npm.cmd");
	});

	it("captures a custom npm prefix and otherwise derives it from the running package", () => {
		expect(
			ResolveNpmPrefix(
				"C:\\ignored\\node_modules\\artisan-editor\\.dist\\entry.js",
				{ npm_config_prefix: "D:\\Portable\\npm-global" },
				"win32",
			),
		).toBe("D:\\Portable\\npm-global");
		expect(
			ResolveNpmPrefix(
				"D:\\Portable\\npm-global\\node_modules\\artisan-editor\\.dist\\entry.js",
				{},
				"win32",
			),
		).toBe("D:\\Portable\\npm-global");
		expect(() =>
			ResolveNpmPrefix("D:\\checkout\\modules\\bootstrap\\.dist\\entry.js", {}, "win32"),
		).toThrow(/Cannot derive the npm prefix/u);
	});

	it("round-trips the detached cleanup plan through a validated argv boundary", async () => {
		const plan: NpmCleanupPlan = {
			executable: "C:\\Program Files\\nodejs\\npm.cmd",
			argv: [
				"uninstall",
				"--global",
				"--prefix",
				"D:\\Portable\\npm-global",
				"artisan-editor",
			],
			bootstrap_pid: 42,
			manual_command:
				'"C:\\Program Files\\nodejs\\npm.cmd" uninstall --global --prefix D:\\Portable\\npm-global artisan-editor',
		};
		const encoded = Buffer.from(JSON.stringify(plan), "utf8").toString("base64url");

		await expect(
			Effect.runPromise(ParseDetachedCleanup(["--artisan-cleanup", encoded])),
		).resolves.toEqual({ _tag: "Cleanup", plan });
	});

	it("fails explicitly when release publication is not configured", async () => {
		const program = Effect.gen(function* () {
			const installer = yield* BootstrapInstaller;
			return yield* installer.InstallFirstTime({
				argv: [],
				bootstrap_pid: 42,
				npm_executable: "C:\\Tools\\npm.cmd",
				npm_prefix: "D:\\Portable\\npm-global",
				package_name: "artisan-editor",
			});
		}).pipe(Effect.provide(BootstrapInstallerUnavailableLive));

		await expect(Effect.runPromise(program)).rejects.toBeInstanceOf(BootstrapInstallFailure);
	});

	it("health-checks and delegates through an absolute executable path", async () => {
		const program = Effect.gen(function* () {
			const permanent_ae = yield* PermanentAe;
			yield* permanent_ae.VerifyHandoff(process.execPath);
			const result = yield* permanent_ae.Execute(process.execPath, "status", ["--version"]);
			const exit_code = yield* permanent_ae.Delegate(process.execPath, ["--version"]);
			return { exit_code, result };
		}).pipe(Effect.provide(make_node_bootstrap_layer(import.meta.filename)));

		await expect(Effect.runPromise(program)).resolves.toMatchObject({
			exit_code: 0,
			result: {
				exit_code: 0,
				stderr: "",
				stderr_truncated: false,
				stdout_truncated: false,
			},
		});
	});

	it("bounds captured permanent ae output and reports truncation", async () => {
		const program = Effect.gen(function* () {
			const permanent_ae = yield* PermanentAe;
			return yield* permanent_ae.Execute(process.execPath, "status", [
				"-e",
				"process.stdout.write('x'.repeat(70000))",
			]);
		}).pipe(Effect.provide(make_node_bootstrap_layer(import.meta.filename)));

		const result = await Effect.runPromise(program);
		expect(result.exit_code).toBe(0);
		expect(Buffer.byteLength(result.stdout, "utf8")).toBe(64 * 1_024);
		expect(result.stdout_truncated).toBe(true);
	});

	it("times out a hung permanent ae command and terminates its process", async () => {
		const program = Effect.gen(function* () {
			const permanent_ae = yield* PermanentAe;
			return yield* permanent_ae.Execute(process.execPath, "status", [
				"-e",
				"setInterval(() => {}, 1000)",
			]);
		}).pipe(Effect.provide(make_node_permanent_ae_layer_with_timeout(50)));

		await expect(Effect.runPromise(program)).rejects.toMatchObject({
			_tag: "PermanentAeFailure",
			operation: "status",
		});
	});
});
