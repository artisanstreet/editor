import { readFile } from "node:fs/promises";

import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import {
	BootstrapCleanup,
	BootstrapCleanupFailure,
	BootstrapInstallationMalformed,
	BootstrapInstaller,
	PermanentAe,
	PermanentAeCommandFailed,
	PermanentAeFailure,
	RunBootstrap,
	type BootstrapInvocation,
	type NpmCleanupPlan,
} from "../../modules/bootstrap/src";
import {
	InstallationStore,
	type InstallationState,
	type ActivatedInstallationManifest,
	type UnactivatedInstallationManifest,
} from "../../modules/distribution/src";

const invocation: BootstrapInvocation = {
	argv: ["open", "--profile", "default"],
	bootstrap_pid: 42,
	npm_executable: "C:\\Program Files\\nodejs\\npm.cmd",
	npm_prefix: "D:\\Portable\\npm-global",
	package_name: "artisan-editor",
};

const ActiveManifest = (): ActivatedInstallationManifest => ({
	format_version: 1,
	install_root: "C:\\Users\\test\\AppData\\Local\\Artisan",
	platform: "windows",
	architecture: "x64",
	channel: "stable",
	activation_state: "active",
	finalization_state: "complete",
	active_version: "0.1.0",
	permanent_ae_path: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe",
	artifact: {
		artifact_id: "artisan-windows-x64",
		sha256: "a".repeat(64),
		signing_key_id: "release-key",
	},
	components: { editor: true, forge: true },
	integrations: {
		ae_path: {
			path: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe",
			fingerprint: "owned-ae",
		},
	},
	transaction: { state: "idle" },
	installed_at: "2026-07-27T00:00:00.000Z",
	updated_at: "2026-07-27T00:00:00.000Z",
});

const PartialManifest = (): UnactivatedInstallationManifest => {
	const active = ActiveManifest();
	const {
		active_version: _,
		artifact: __,
		permanent_ae_path: ___,
		previous_version: ____,
		finalization_state: _____,
		...common
	} = active;

	return {
		...common,
		activation_state: "unactivated",
		integrations: {},
		transaction: {
			state: "staged",
			target_version: "0.1.0",
			staging_path: "C:\\Users\\test\\AppData\\Local\\Artisan\\staging\\0.1.0",
			started_at: "2026-07-27T00:00:00.000Z",
		},
	};
};

const Execute = (
	state: InstallationState,
	options: {
		readonly command_failure?: "delegate" | "setup" | "start" | "status";
		readonly cleanup_fails?: boolean;
		readonly events?: Array<string>;
		readonly status_stdout?: string;
		readonly status_stdout_truncated?: boolean;
		readonly verify_fails?: boolean;
	} = {},
) => {
	const events = options.events ?? [];
	const handoff_path = "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe";
	let current_state = state;
	const ActivatePending = () => {
		const active = ActiveManifest();
		current_state = {
			_tag: "Partial",
			manifest: { ...active, finalization_state: "pending" },
		};
	};

	return RunBootstrap(invocation).pipe(
		Effect.provideService(InstallationStore, {
			Inspect: () => Effect.succeed(current_state),
			WriteAtomic: (manifest) =>
				Effect.sync(() => {
					current_state =
						manifest.activation_state === "active" &&
						manifest.transaction.state === "idle" &&
						manifest.finalization_state === "complete"
							? { _tag: "Healthy", manifest }
							: { _tag: "Partial", manifest };
					events.push(
						`manifest:${manifest.activation_state}:${
							manifest.activation_state === "active"
								? (manifest.finalization_state ?? "legacy")
								: "unactivated"
						}`,
					);
				}),
		}),
		Effect.provideService(BootstrapInstaller, {
			InstallFirstTime: () =>
				Effect.sync(() => {
					events.push("install");
					ActivatePending();
					return { permanent_ae_path: handoff_path };
				}),
			Resume: () =>
				Effect.sync(() => {
					events.push("resume");
					ActivatePending();
					return { permanent_ae_path: handoff_path };
				}),
		}),
		Effect.provideService(PermanentAe, {
			VerifyHandoff: (permanent_ae_path) =>
				Effect.sync(() => events.push(`verify:${permanent_ae_path}`)).pipe(
					Effect.flatMap(() =>
						options.verify_fails
							? Effect.fail(
									new PermanentAeFailure({
										cause: "unhealthy",
										operation: "verify",
										permanent_ae_path,
									}),
								)
							: Effect.void,
					),
				),
			Delegate: (permanent_ae_path, argv) =>
				Effect.sync(() => {
					events.push(`delegate:${permanent_ae_path}:${argv.join("|")}`);
					return options.command_failure === "delegate" ? 7 : 0;
				}),
			Execute: (permanent_ae_path, operation, argv) =>
				Effect.sync(() => {
					events.push(`${operation}:${permanent_ae_path}:${argv.join("|")}`);
					return {
						exit_code: options.command_failure === operation ? 7 : 0,
						stdout:
							operation === "status"
								? (options.status_stdout ?? '{"state":"running"}')
								: "",
						stderr: options.command_failure === operation ? "command failed" : "",
						stdout_truncated:
							operation === "status" && (options.status_stdout_truncated ?? false),
						stderr_truncated: false,
					};
				}),
		}),
		Effect.provideService(BootstrapCleanup, {
			ScheduleDetached: (plan: NpmCleanupPlan) =>
				Effect.sync(() => events.push(`cleanup:${plan.manual_command}`)).pipe(
					Effect.flatMap(() =>
						options.cleanup_fails
							? Effect.fail(
									new BootstrapCleanupFailure({
										cause: "scheduler unavailable",
										plan,
									}),
								)
							: Effect.void,
					),
				),
		}),
	);
};

describe("disposable bootstrap", () => {
	it("installs from an absent state, verifies before cleanup, and delegates absolutely", async () => {
		const events: Array<string> = [];
		const outcome = await Effect.runPromise(Execute({ _tag: "Absent" }, { events }));

		expect(outcome).toMatchObject({
			route: "installed",
			permanent_ae_path: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe",
			exit_code: 0,
			cleanup: { state: "scheduled" },
		});
		expect(events).toEqual([
			"install",
			"verify:C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe",
			"setup:C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe:setup|--profile|default",
			"start:C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe:start|--profile|default",
			"status:C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe:status|--profile|default|--json",
			"manifest:active:complete",
			"delegate:C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe:open|--profile|default",
			'cleanup:"C:\\Program Files\\nodejs\\npm.cmd" uninstall --global --prefix D:\\Portable\\npm-global artisan-editor',
		]);
	});

	it("resumes a partial installation instead of layering a second install", async () => {
		const events: Array<string> = [];
		const outcome = await Effect.runPromise(
			Execute({ _tag: "Partial", manifest: PartialManifest() }, { events }),
		);

		expect(outcome.route).toBe("resumed");
		expect(events[0]).toBe("resume");
		expect(events).not.toContain("install");
	});

	it("delegates a healthy installation from its recorded absolute ae path", async () => {
		const events: Array<string> = [];
		const outcome = await Effect.runPromise(
			Execute({ _tag: "Healthy", manifest: ActiveManifest() }, { events }),
		);

		expect(outcome.route).toBe("delegated");
		expect(events.some((event) => event === "install" || event === "resume")).toBe(false);
		expect(events.some((event) => event.startsWith("setup:"))).toBe(false);
		expect(events.at(-2)).toContain(
			"delegate:C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe",
		);
	});

	it("does not repair malformed state implicitly", async () => {
		const events: Array<string> = [];

		await expect(
			Effect.runPromise(
				Execute(
					{
						_tag: "Malformed",
						cause: "invalid json",
						manifest_path:
							"C:\\Users\\test\\AppData\\Local\\Artisan\\installation.json",
					},
					{ events },
				),
			),
		).rejects.toBeInstanceOf(BootstrapInstallationMalformed);
		expect(events).toEqual([]);
	});

	it("reports an exact nonfatal npm fallback when detached cleanup cannot be scheduled", async () => {
		const outcome = await Effect.runPromise(
			Execute({ _tag: "Absent" }, { cleanup_fails: true }),
		);

		expect(outcome.cleanup).toEqual({
			state: "manual",
			command:
				'"C:\\Program Files\\nodejs\\npm.cmd" uninstall --global --prefix D:\\Portable\\npm-global artisan-editor',
		});
		expect(outcome.exit_code).toBe(0);
	});

	it.each(["setup", "start", "status", "delegate"] as const)(
		"preserves the bootstrap when permanent ae %s fails",
		async (operation) => {
			const events: Array<string> = [];

			await expect(
				Effect.runPromise(
					Execute({ _tag: "Absent" }, { command_failure: operation, events }),
				),
			).rejects.toMatchObject({
				_tag: "PermanentAeCommandFailed",
				exit_code: 7,
				operation,
			} satisfies Partial<PermanentAeCommandFailed>);
			expect(events.some((event) => event.startsWith("cleanup:"))).toBe(false);
		},
	);

	it.each([
		["malformed", "not-json", false],
		["not running", '{"state":"stopped"}', false],
		["truncated", '{"state":"running"}', true],
	] as const)(
		"keeps first-run pending when Forge status is %s",
		async (_label, status_stdout, status_stdout_truncated) => {
			const events: Array<string> = [];
			await expect(
				Effect.runPromise(
					Execute({ _tag: "Absent" }, { events, status_stdout, status_stdout_truncated }),
				),
			).rejects.toMatchObject({ _tag: "PermanentAeStatusInvalid" });
			expect(events).not.toContain("manifest:active:complete");
			expect(events.some((event) => event.startsWith("cleanup:"))).toBe(false);
		},
	);

	it("never schedules removal when the permanent handoff is unhealthy", async () => {
		const events: Array<string> = [];

		await expect(
			Effect.runPromise(Execute({ _tag: "Absent" }, { events, verify_fails: true })),
		).rejects.toBeInstanceOf(PermanentAeFailure);
		expect(events).toEqual([
			"install",
			"verify:C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.exe",
		]);
	});

	it("ships an ae bin without an npm postinstall hook", async () => {
		const package_json = JSON.parse(
			await readFile("modules/bootstrap/package.json", "utf8"),
		) as {
			readonly bin?: Record<string, string>;
			readonly dependencies?: Record<string, string>;
			readonly name?: string;
			readonly os?: ReadonlyArray<string>;
			readonly private?: boolean;
			readonly scripts?: Record<string, string>;
		};

		expect(package_json.name).toBe("artisan-editor");
		expect(package_json.private).not.toBe(true);
		expect(package_json.os).toEqual(["win32"]);
		expect(
			Object.values(package_json.dependencies ?? {}).some((value) =>
				value.startsWith("workspace:"),
			),
		).toBe(false);
		expect(package_json.bin).toEqual({ ae: "./.dist/entry.js" });
		expect(package_json.scripts?.postinstall).toBeUndefined();
	});
});
