import {
	InstallationStore,
	Installer,
	IntegrationLifecycle,
	type ActivatedInstallationManifest,
} from "@artisan/distribution";
import { describe, expect, it } from "vitest";
import { Effect, Layer, ManagedRuntime } from "effect";

import {
	DistributionIntegrationPlan,
	DistributionOperations,
	DistributionOperationsLive,
	DistributionRemoval,
} from "../../modules/cli/src/distribution";

const manifest: ActivatedInstallationManifest = {
	format_version: 1,
	install_root: "C:\\Artisan",
	platform: "windows",
	architecture: "x64",
	channel: "stable",
	components: { editor: true, forge: true },
	integrations: {
		protocol: {
			path: "C:\\Artisan\\protocol",
			fingerprint: "owned",
		},
	},
	installed_at: "2026-07-27T00:00:00.000Z",
	updated_at: "2026-07-27T00:00:00.000Z",
	activation_state: "active",
	active_version: "1.0.0",
	permanent_ae_path: "C:\\Artisan\\bin\\ae.exe",
	artifact: {
		artifact_id: "artisan-windows-x64",
		sha256: "a".repeat(64),
		signing_key_id: "release",
	},
	transaction: { state: "idle" },
};

const MakeRuntime = (integration_state: "Drifted" | "Missing" | "Owned" = "Owned") => {
	let current_manifest = manifest;
	let forge_data_removals = 0;
	let product_removals = 0;
	let repairs = 0;
	let uninstalls = 0;
	const store = Layer.succeed(
		InstallationStore,
		InstallationStore.of({
			Inspect: () => Effect.succeed({ _tag: "Healthy", manifest: current_manifest }),
			WriteAtomic: (next) =>
				Effect.sync(() => {
					if (next.activation_state === "active") current_manifest = next;
				}),
		}),
	);
	const integrations = Layer.succeed(
		IntegrationLifecycle,
		IntegrationLifecycle.of({
			Inspect: () =>
				Effect.succeed([
					integration_state === "Drifted"
						? {
								_tag: "Drifted",
								actual_fingerprint: "foreign",
								expected_fingerprint: "owned",
								kind: "protocol",
								path: "C:\\Artisan\\protocol",
							}
						: {
								_tag: integration_state,
								kind: "protocol",
								path: "C:\\Artisan\\protocol",
							},
				]),
			Install: () => Effect.succeed({}),
			Repair: () =>
				Effect.sync(() => {
					repairs += 1;
					return current_manifest.integrations;
				}),
			Uninstall: () =>
				Effect.sync(() => {
					uninstalls += 1;
					return integration_state === "Owned"
						? [{ _tag: "Removed", kind: "protocol", path: "C:\\Artisan\\protocol" }]
						: [
								{
									_tag: "Preserved",
									kind: "protocol",
									path: "C:\\Artisan\\protocol",
									reason:
										integration_state === "Missing"
											? "missing"
											: "ownership_mismatch",
								},
							];
				}),
		}),
	);
	const installer = Layer.succeed(
		Installer,
		Installer.of({
			Converge: () => Effect.succeed({ _tag: "Current", manifest: current_manifest }),
		}),
	);
	const plan = Layer.succeed(
		DistributionIntegrationPlan,
		DistributionIntegrationPlan.of({
			Specifications: () =>
				Effect.succeed([
					{
						kind: "protocol",
						path: "C:\\Artisan\\protocol",
						content: "artisan protocol",
					},
				]),
		}),
	);
	const removal = Layer.succeed(
		DistributionRemoval,
		DistributionRemoval.of({
			RemoveForgeData: () =>
				Effect.sync(() => {
					forge_data_removals += 1;
				}),
			RemoveProduct: () =>
				Effect.sync(() => {
					product_removals += 1;
					return { manual_cleanup_command: "cleanup fixture" };
				}),
		}),
	);
	return {
		counts: () => ({ forge_data_removals, product_removals, repairs, uninstalls }),
		runtime: ManagedRuntime.make(
			DistributionOperationsLive.pipe(
				Layer.provide(Layer.mergeAll(store, integrations, installer, plan, removal)),
			),
		),
	};
};

describe("permanent ae distribution lifecycle", () => {
	it("reports installation and owned integration health", async () => {
		const { runtime } = MakeRuntime();
		const operations = await runtime.runPromise(DistributionOperations);
		expect(await runtime.runPromise(operations.Doctor())).toMatchObject({
			healthy: true,
			installation: "Healthy",
		});
		await runtime.dispose();
	});

	it("repairs integrations from the installation-owned specification", async () => {
		const { runtime, counts } = MakeRuntime("Missing");
		const operations = await runtime.runPromise(DistributionOperations);
		await runtime.runPromise(operations.Repair());
		expect(counts().repairs).toBe(1);
		await runtime.dispose();
	});

	it("delegates updates to the signed installer convergence service", async () => {
		const { runtime } = MakeRuntime();
		const operations = await runtime.runPromise(DistributionOperations);
		expect(await runtime.runPromise(operations.Update())).toMatchObject({
			_tag: "Current",
			manifest: { active_version: "1.0.0" },
		});
		await runtime.dispose();
	});

	it("retains Forge data by default and preserves drifted integrations", async () => {
		const { runtime, counts } = MakeRuntime("Drifted");
		const operations = await runtime.runPromise(DistributionOperations);
		const report = await runtime.runPromise(operations.Uninstall(false));
		expect(report).toMatchObject({
			forge_data_removed: false,
			integrations: [{ _tag: "Preserved", reason: "ownership_mismatch" }],
		});
		expect(counts()).toEqual({
			forge_data_removals: 0,
			product_removals: 1,
			repairs: 0,
			uninstalls: 1,
		});
		await runtime.dispose();
	});

	it("removes Forge data only with explicit destructive authority", async () => {
		const { runtime, counts } = MakeRuntime();
		const operations = await runtime.runPromise(DistributionOperations);
		await runtime.runPromise(operations.Uninstall(true));
		expect(counts().forge_data_removals).toBe(1);
		await runtime.dispose();
	});
});
