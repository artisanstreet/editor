import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { afterEach, describe, expect, it } from "vitest";
import { Effect, Layer, ManagedRuntime } from "effect";

import {
	BootstrapCleanup,
	BootstrapInstaller,
	PermanentAe,
	RunBootstrap,
	type BootstrapInvocation,
} from "../../../modules/installer/src";
import {
	DistributionIntegrationPlan,
	DistributionOperations,
	DistributionOperationsLive,
	DistributionRemoval,
} from "../../../modules/cli/src/distribution";
import {
	Activation,
	ArtifactDownloader,
	ArtifactStager,
	ArtifactStagingFailure,
	ForgeUpdateLifecycle,
	InstallationHealth,
	InstallationHealthFailure,
	InstallationIntegrations,
	IntegrationFailure,
	InstallationStore,
	Installer,
	IntegrationLifecycle,
	IntegrationLifecycleLive,
	ReleaseSource,
	ReleaseVerification,
	WindowsIntegrationAdapterLive,
	WindowsIntegrationPlatformLive,
	WindowsNativeHost,
	make_activation_layer,
	make_installation_store_layer,
	make_installer_layer,
	type IntegrationSpecification,
	type ReleaseArtifact,
	type ReleaseManifest,
	type WindowsCommandResult,
} from "../../../modules/distribution/src";

const temporary_roots: Array<string> = [];
const artifact_bytes = new Uint8Array([1, 2, 3]);
const sha256 = "a".repeat(64);

afterEach(async () => {
	await Promise.all(
		temporary_roots.splice(0).map((root) => rm(root, { force: true, recursive: true })),
	);
});

const MakeRelease = (version: string): ReleaseManifest => ({
	format_version: 1,
	product_version: version,
	editor_forge_compatibility_version: version,
	channel: "stable",
	signing_identity: { algorithm: "ed25519", key_id: "release-key" },
	minimum_installer_version: "0.1.0",
	minimum_cli_version: "0.1.0",
	artifacts: [
		{
			artifact_id: `artisan-windows-x64-${version}`,
			platform: "windows",
			architecture: "x64",
			archive_format: "zip",
			file_name: `artisan-${version}.zip`,
			byte_size: artifact_bytes.byteLength,
			sha256,
			archive_entries: ["bin/ae.exe", "editor/Artisan Editor.exe", "forge/Artisan Forge.exe"],
		},
	],
});

interface NativeState {
	path: string;
	readonly registry: Map<string, string>;
	readonly shortcuts: Map<string, string>;
	readonly tasks: Map<string, string>;
}

const Result = (stdout = "", exit_code = 0, stderr = ""): WindowsCommandResult => ({
	exit_code,
	stderr,
	stdout,
});

const RegistryValueKey = (key: string, value_name: string | undefined) =>
	`${key}:${value_name ?? "(Default)"}`;

const MakeNativeHost = (state: NativeState) =>
	WindowsNativeHost.of({
		Run: (executable, arguments_, environment = {}) =>
			Effect.sync(() => {
				if (executable === "reg.exe") {
					const operation = arguments_[0];
					const key = arguments_[1] ?? "";
					const value_flag = arguments_.indexOf("/v");
					const value_name = value_flag === -1 ? undefined : arguments_[value_flag + 1];
					if (operation === "query") {
						const value =
							key === "HKCU\\Environment"
								? state.path
								: state.registry.get(RegistryValueKey(key, value_name));
						return value === undefined || value === ""
							? Result("", 1)
							: Result(`    ${value_name ?? "(Default)"}    REG_SZ    ${value}\r\n`);
					}
					if (operation === "add") {
						const data_index = arguments_.indexOf("/d");
						const value = arguments_[data_index + 1] ?? "";
						if (key === "HKCU\\Environment") state.path = value;
						else state.registry.set(RegistryValueKey(key, value_name), value);
						return Result();
					}
					if (operation === "delete") {
						for (const stored_key of state.registry.keys()) {
							if (stored_key.startsWith(`${key}:`)) state.registry.delete(stored_key);
						}
						return Result();
					}
				}

				if (executable === "powershell.exe") {
					const shortcut = environment.ARTISAN_SHORTCUT;
					if (shortcut !== undefined) {
						if (environment.ARTISAN_CONTENT !== undefined) {
							state.shortcuts.set(shortcut, environment.ARTISAN_CONTENT);
							return Result();
						}
						if (arguments_.includes("-Command")) {
							state.shortcuts.delete(shortcut);
							return Result();
						}
						const content = state.shortcuts.get(shortcut);
						return content === undefined ? Result("", 1) : Result(content);
					}
					const task = environment.ARTISAN_TASK;
					if (task !== undefined) {
						if (environment.ARTISAN_CONTENT !== undefined) {
							state.tasks.set(task, environment.ARTISAN_CONTENT);
							return Result();
						}
						const content = state.tasks.get(task);
						return content === undefined ? Result("", 1) : Result(content);
					}
					return Result();
				}

				if (executable === "schtasks.exe") {
					const task_index = arguments_.indexOf("/TN");
					const task = arguments_[task_index + 1];
					if (task !== undefined) state.tasks.delete(task);
					return Result();
				}
				return Result("", 1, `Unexpected executable: ${executable}`);
			}),
	});

interface Harness {
	readonly activation: Activation["Service"];
	readonly data_root: string;
	readonly events: Array<string>;
	health_failure_version: string | undefined;
	readonly installer: Installer["Service"];
	readonly native: NativeState;
	readonly operations: DistributionOperations["Service"];
	readonly permanent_ae_path: string;
	product_removals: number;
	release: ReleaseManifest;
	remaining_staging_failures: number;
	readonly root: string;
	readonly runtime: { readonly dispose: () => Promise<void> };
	readonly store: InstallationStore["Service"];
}

const MakeHarness = async (): Promise<Harness> => {
	const root = await mkdtemp(join(tmpdir(), "artisan-windows-acceptance-"));
	temporary_roots.push(root);
	const data_root = join(dirname(root), `${root.split("\\").at(-1)}-forge-data`);
	temporary_roots.push(data_root);
	await mkdir(data_root, { recursive: true });
	await writeFile(join(data_root, "artisan.sqlite"), "retained");

	const permanent_ae_path = join(root, "bin", "ae.exe");
	const bin_path = dirname(permanent_ae_path);
	const editor_path = join(root, "editor", "Artisan Editor.exe");
	const specifications: ReadonlyArray<IntegrationSpecification> = [
		{ kind: "ae_path", path: bin_path, content: `PATH:${bin_path}` },
		{ kind: "protocol", path: editor_path, content: `"${editor_path}" "%1"` },
	];
	const native: NativeState = {
		path: "C:\\Foreign",
		registry: new Map(),
		shortcuts: new Map(),
		tasks: new Map(),
	};
	const events: Array<string> = [];
	const mutable = {
		health_failure_version: undefined as string | undefined,
		product_removals: 0,
		release: MakeRelease("0.1.0"),
		remaining_staging_failures: 0,
	};

	const platform = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);
	const store_layer = make_installation_store_layer(root).pipe(Layer.provide(platform));
	const activation_layer = make_activation_layer(root).pipe(Layer.provide(platform));
	const native_host_layer = Layer.succeed(WindowsNativeHost, MakeNativeHost(native));
	const windows_adapter_layer = WindowsIntegrationAdapterLive.pipe(
		Layer.provide(native_host_layer),
	);
	const integration_platform_layer = WindowsIntegrationPlatformLive.pipe(
		Layer.provide(windows_adapter_layer),
	);
	const lifecycle_layer = IntegrationLifecycleLive.pipe(
		Layer.provide(integration_platform_layer),
	);
	const release_adapters = Layer.mergeAll(
		Layer.succeed(
			ReleaseSource,
			ReleaseSource.of({
				Resolve: () =>
					Effect.sync(() => ({
						manifest: new TextEncoder().encode(JSON.stringify(mutable.release)),
						signature: {},
					})),
			}),
		),
		Layer.succeed(
			ArtifactDownloader,
			ArtifactDownloader.of({ Download: () => Effect.succeed(artifact_bytes) }),
		),
		Layer.succeed(
			ReleaseVerification,
			ReleaseVerification.of({
				VerifyArtifact: (_artifact: ReleaseArtifact, _bytes: Uint8Array) => Effect.void,
				VerifyManifest: () => Effect.sync(() => mutable.release),
			}),
		),
		Layer.succeed(
			ArtifactStager,
			ArtifactStager.of({
				Stage: ({ version_path }) =>
					mutable.remaining_staging_failures > 0
						? Effect.sync(() => {
								mutable.remaining_staging_failures -= 1;
							}).pipe(
								Effect.andThen(
									Effect.fail(
										new ArtifactStagingFailure({
											cause: new Error("interrupted extraction"),
											target_version: mutable.release.product_version,
										}),
									),
								),
							)
						: Effect.tryPromise({
								try: async () => {
									await mkdir(version_path, { recursive: true });
									await writeFile(join(version_path, "ae.exe"), "fixture");
								},
								catch: (cause) =>
									new ArtifactStagingFailure({
										cause,
										target_version: mutable.release.product_version,
									}),
							}),
			}),
		),
		Layer.succeed(
			InstallationHealth,
			InstallationHealth.of({
				Check: (version) =>
					mutable.health_failure_version === version
						? Effect.fail(
								new InstallationHealthFailure({
									cause: new Error("fixture health failure"),
									version,
								}),
							)
						: Effect.void,
			}),
		),
		Layer.succeed(
			ForgeUpdateLifecycle,
			ForgeUpdateLifecycle.of({
				Quiesce: () => Effect.succeed({ was_running: false }),
				Restore: () => Effect.void,
				ResumeAndVerify: () => Effect.void,
				VerifyCurrent: () => Effect.void,
			}),
		),
	);
	const installation_integrations_layer = Layer.effect(
		InstallationIntegrations,
		Effect.gen(function* () {
			const lifecycle = yield* IntegrationLifecycle;
			return InstallationIntegrations.of({
				Apply: ({ release }) =>
					lifecycle.Install(specifications).pipe(
						Effect.mapError(
							(cause) =>
								new IntegrationFailure({
									cause,
									target_version: release.product_version,
								}),
						),
					),
			});
		}),
	).pipe(Layer.provide(lifecycle_layer));
	const installer_dependencies = Layer.mergeAll(
		platform,
		store_layer,
		activation_layer,
		release_adapters,
		installation_integrations_layer,
	);
	const installer_layer = make_installer_layer({
		install_root: root,
		permanent_ae_path,
		platform: "windows",
		architecture: "x64",
		channel: "stable",
		bootstrap_version: "0.1.0",
		cli_version: "0.1.0",
		components: { editor: true, forge: true },
	}).pipe(Layer.provide(installer_dependencies));
	const plan_layer = Layer.succeed(
		DistributionIntegrationPlan,
		DistributionIntegrationPlan.of({
			Specifications: () => Effect.succeed(specifications),
		}),
	);
	const removal_layer = Layer.succeed(
		DistributionRemoval,
		DistributionRemoval.of({
			RemoveForgeData: () =>
				Effect.tryPromise({
					try: () => rm(data_root, { force: true, recursive: true }),
					catch: (cause) => cause as never,
				}),
			RemoveProduct: () =>
				Effect.sync(() => {
					mutable.product_removals += 1;
					return { manual_cleanup_command: "cleanup fixture" };
				}),
		}),
	);
	const operations_layer = DistributionOperationsLive.pipe(
		Layer.provide(
			Layer.mergeAll(
				store_layer,
				installer_layer,
				lifecycle_layer,
				plan_layer,
				removal_layer,
			),
		),
	);
	const runtime = ManagedRuntime.make(
		Layer.mergeAll(
			store_layer,
			activation_layer,
			lifecycle_layer,
			installer_layer,
			operations_layer,
		),
	);

	const harness: Harness = {
		activation: await runtime.runPromise(Activation),
		data_root,
		events,
		get health_failure_version() {
			return mutable.health_failure_version;
		},
		set health_failure_version(value) {
			mutable.health_failure_version = value;
		},
		installer: await runtime.runPromise(Installer),
		native,
		operations: await runtime.runPromise(DistributionOperations),
		permanent_ae_path,
		get product_removals() {
			return mutable.product_removals;
		},
		set product_removals(value) {
			mutable.product_removals = value;
		},
		get release() {
			return mutable.release;
		},
		set release(value) {
			mutable.release = value;
		},
		get remaining_staging_failures() {
			return mutable.remaining_staging_failures;
		},
		set remaining_staging_failures(value) {
			mutable.remaining_staging_failures = value;
		},
		root,
		runtime,
		store: await runtime.runPromise(InstallationStore),
	};
	return harness;
};

const invocation: BootstrapInvocation = {
	argv: ["status"],
	bootstrap_pid: 42,
	npm_executable: "C:\\Program Files\\nodejs\\npm.cmd",
	npm_prefix: "D:\\Portable\\npm-global",
	package_name: "artisan-editor",
};

const RunHarnessBootstrap = (harness: Harness) => {
	let install_calls = 0;
	const program = RunBootstrap(invocation).pipe(
		Effect.provideService(InstallationStore, harness.store),
		Effect.provideService(BootstrapInstaller, {
			InstallFirstTime: () =>
				Effect.sync(() => {
					install_calls += 1;
				}).pipe(
					Effect.andThen(harness.installer.Converge()),
					Effect.map((outcome) => ({
						permanent_ae_path: outcome.manifest.permanent_ae_path,
					})),
					Effect.mapError((cause) => cause as never),
				),
			Resume: () =>
				Effect.sync(() => {
					install_calls += 1;
				}).pipe(
					Effect.andThen(harness.installer.Converge()),
					Effect.map((outcome) => ({
						permanent_ae_path: outcome.manifest.permanent_ae_path,
					})),
					Effect.mapError((cause) => cause as never),
				),
		}),
		Effect.provideService(PermanentAe, {
			VerifyHandoff: (path) =>
				Effect.sync(() => {
					harness.events.push(`health:${path}`);
				}),
			Delegate: (path, argv) =>
				Effect.sync(() => {
					harness.events.push(`delegate:${path}:${argv.join("|")}`);
					return 0;
				}),
			Execute: (path, operation, argv) =>
				Effect.sync(() => {
					harness.events.push(`${operation}:${path}:${argv.join("|")}`);
					return {
						exit_code: 0,
						stdout: operation === "status" ? '{"state":"running"}' : "",
						stderr: "",
						stdout_truncated: false,
						stderr_truncated: false,
					};
				}),
		}),
		Effect.provideService(BootstrapCleanup, {
			ScheduleDetached: () =>
				Effect.sync(() => {
					harness.events.push("cleanup");
				}),
		}),
	);
	return { install_calls: () => install_calls, program };
};

describe("hermetic Windows distribution lifecycle", () => {
	it("performs a clean first install and publishes owned Windows integrations", async () => {
		const harness = await MakeHarness();
		const bootstrap = RunHarnessBootstrap(harness);

		await expect(Effect.runPromise(bootstrap.program)).resolves.toMatchObject({
			route: "installed",
			permanent_ae_path: harness.permanent_ae_path,
			exit_code: 0,
		});
		expect(await Effect.runPromise(harness.store.Inspect())).toMatchObject({
			_tag: "Healthy",
			manifest: {
				activation_state: "active",
				active_version: "0.1.0",
				permanent_ae_path: harness.permanent_ae_path,
				transaction: { state: "idle" },
			},
		});
		expect(harness.native.path).toContain(join(harness.root, "bin"));
		expect(
			harness.native.registry.get(
				"HKCU\\Software\\Classes\\artisan\\shell\\open\\command:(Default)",
			),
		).toBe(`"${join(harness.root, "editor", "Artisan Editor.exe")}" "%1"`);
		await harness.runtime.dispose();
	});

	it("resumes first install and removes npm only after permanent ae is healthy", async () => {
		const harness = await MakeHarness();
		harness.remaining_staging_failures = 1;
		const first = RunHarnessBootstrap(harness);

		await expect(Effect.runPromise(first.program)).rejects.toBeInstanceOf(
			ArtifactStagingFailure,
		);
		expect(harness.events).toEqual([]);
		expect(await Effect.runPromise(harness.store.Inspect())).toMatchObject({
			_tag: "Partial",
			manifest: { transaction: { state: "verified" } },
		});

		const resumed = RunHarnessBootstrap(harness);
		await expect(Effect.runPromise(resumed.program)).resolves.toMatchObject({
			route: "resumed",
			permanent_ae_path: harness.permanent_ae_path,
			cleanup: { state: "scheduled" },
		});
		expect(harness.events).toEqual([
			`health:${harness.permanent_ae_path}`,
			`setup:${harness.permanent_ae_path}:setup`,
			`start:${harness.permanent_ae_path}:start`,
			`status:${harness.permanent_ae_path}:status|--json`,
			`delegate:${harness.permanent_ae_path}:status`,
			"cleanup",
		]);
		expect(harness.native.path).toContain(join(harness.root, "bin"));

		harness.events.splice(0);
		const healthy = RunHarnessBootstrap(harness);
		await expect(Effect.runPromise(healthy.program)).resolves.toMatchObject({
			route: "delegated",
		});
		expect(healthy.install_calls()).toBe(0);
		expect(harness.events[0]).toBe(`health:${harness.permanent_ae_path}`);
		await harness.runtime.dispose();
	});

	it("updates side-by-side and restores the prior version after failed health", async () => {
		const harness = await MakeHarness();
		await Effect.runPromise(RunHarnessBootstrap(harness).program);

		harness.release = MakeRelease("0.2.0");
		await expect(Effect.runPromise(harness.operations.Update())).resolves.toMatchObject({
			_tag: "Updated",
			manifest: { active_version: "0.2.0", previous_version: "0.1.0" },
		});

		const cleanup_count = harness.events.filter((event) => event === "cleanup").length;
		harness.release = MakeRelease("0.3.0");
		harness.health_failure_version = "0.3.0";
		await expect(Effect.runPromise(harness.operations.Update())).rejects.toMatchObject({
			_tag: "DistributionOperationsError",
		});
		expect(await Effect.runPromise(harness.activation.ReadActive())).toMatchObject({
			_tag: "Active",
			pointer: { active_version: "0.2.0" },
		});
		expect(await Effect.runPromise(harness.store.Inspect())).toMatchObject({
			_tag: "Healthy",
			manifest: { active_version: "0.2.0", transaction: { state: "idle" } },
		});
		expect(harness.events.filter((event) => event === "cleanup")).toHaveLength(cleanup_count);
		await harness.runtime.dispose();
	});

	it("repairs missing owned state, preserves drift, and gates data destruction explicitly", async () => {
		const harness = await MakeHarness();
		await Effect.runPromise(RunHarnessBootstrap(harness).program);
		const protocol_key = "HKCU\\Software\\Classes\\artisan\\shell\\open\\command:(Default)";

		harness.native.path = "C:\\Foreign";
		await expect(Effect.runPromise(harness.operations.Repair())).resolves.toMatchObject({
			healthy: true,
		});
		expect(harness.native.path).toContain(join(harness.root, "bin"));

		harness.native.registry.set(protocol_key, '"C:\\Foreign\\Other.exe" "%1"');
		const repaired = await Effect.runPromise(harness.operations.Repair());
		expect(repaired.healthy).toBe(false);
		expect(repaired.integrations).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ _tag: "Drifted", kind: "protocol" }),
			]),
		);
		expect(harness.native.registry.get(protocol_key)).toBe('"C:\\Foreign\\Other.exe" "%1"');

		const retained = await Effect.runPromise(harness.operations.Uninstall(false));
		expect(retained.forge_data_removed).toBe(false);
		expect(retained.integrations).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					_tag: "Preserved",
					kind: "protocol",
					reason: "ownership_mismatch",
				}),
			]),
		);
		await expect(
			import("node:fs/promises").then(({ stat }) => stat(harness.data_root)),
		).resolves.toBeDefined();

		const destructive = await Effect.runPromise(harness.operations.Uninstall(true));
		expect(destructive.forge_data_removed).toBe(true);
		await expect(
			import("node:fs/promises").then(({ stat }) => stat(harness.data_root)),
		).rejects.toMatchObject({ code: "ENOENT" });
		expect(harness.product_removals).toBe(2);
		await harness.runtime.dispose();
	});
});
