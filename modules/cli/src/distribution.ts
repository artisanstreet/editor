import {
	InstallationStore,
	type InstallationState,
	Installer,
	type InstallerOutcome,
	IntegrationLifecycle,
	type IntegrationSpecification,
	type IntegrationState,
	type InstallationManifest,
} from "@artisan/distribution";
import { Context, Data, Effect, Layer } from "effect";

export class DistributionOperationsError extends Data.TaggedError("DistributionOperationsError")<{
	readonly cause?: unknown;
	readonly code:
		| "installation_absent"
		| "installation_malformed"
		| "installation_partial"
		| "operation_unavailable";
	readonly operation: "doctor" | "repair" | "uninstall" | "update";
}> {}

export interface DistributionDoctorReport {
	readonly healthy: boolean;
	readonly installation: InstallationState["_tag"];
	readonly integrations: ReadonlyArray<IntegrationState>;
}

export interface DistributionUninstallReport {
	readonly forge_data_removed: boolean;
	readonly manual_cleanup_command: string;
	readonly integrations: ReadonlyArray<{
		readonly _tag: "Preserved" | "Removed";
		readonly kind: string;
		readonly path: string;
		readonly reason?: "missing" | "ownership_mismatch";
	}>;
}

/** Computes the exact desired integration content for an installed version. */
export class DistributionIntegrationPlan extends Context.Service<
	DistributionIntegrationPlan,
	{
		readonly Specifications: (
			manifest: InstallationManifest,
		) => Effect.Effect<ReadonlyArray<IntegrationSpecification>, DistributionOperationsError>;
	}
>()("Artisan/Cli/DistributionIntegrationPlan") {}

/** Destructive removal remains an explicit injected platform capability. */
export class DistributionRemoval extends Context.Service<
	DistributionRemoval,
	{
		readonly RemoveForgeData: () => Effect.Effect<void, DistributionOperationsError>;
		readonly RemoveProduct: (
			manifest: InstallationManifest,
		) => Effect.Effect<
			{ readonly manual_cleanup_command: string },
			DistributionOperationsError
		>;
	}
>()("Artisan/Cli/DistributionRemoval") {}

export class DistributionOperations extends Context.Service<
	DistributionOperations,
	{
		readonly Doctor: () => Effect.Effect<DistributionDoctorReport, DistributionOperationsError>;
		readonly Repair: () => Effect.Effect<DistributionDoctorReport, DistributionOperationsError>;
		readonly Uninstall: (
			remove_data: boolean,
		) => Effect.Effect<DistributionUninstallReport, DistributionOperationsError>;
		readonly Update: () => Effect.Effect<InstallerOutcome, DistributionOperationsError>;
	}
>()("Artisan/Cli/DistributionOperations") {}

const MapFailure = (operation: DistributionOperationsError["operation"]) => (cause: unknown) =>
	cause instanceof DistributionOperationsError
		? cause
		: new DistributionOperationsError({
				cause,
				code: "operation_unavailable",
				operation,
			});

const RequireActive = (
	state: InstallationState,
	operation: DistributionOperationsError["operation"],
) =>
	state._tag === "Healthy" && state.manifest.activation_state === "active"
		? Effect.succeed(state.manifest)
		: Effect.fail(
				new DistributionOperationsError({
					code:
						state._tag === "Absent"
							? "installation_absent"
							: state._tag === "Malformed"
								? "installation_malformed"
								: "installation_partial",
					operation,
					...(state._tag === "Malformed" ? { cause: state.cause } : {}),
				}),
			);

export const DistributionOperationsLive = Layer.effect(
	DistributionOperations,
	Effect.gen(function* () {
		const store = yield* InstallationStore;
		const installer = yield* Installer;
		const integrations = yield* IntegrationLifecycle;
		const integration_plan = yield* DistributionIntegrationPlan;
		const removal = yield* DistributionRemoval;

		const Doctor = () =>
			Effect.gen(function* () {
				const state = yield* store.Inspect();
				if (state._tag !== "Healthy" || state.manifest.activation_state !== "active") {
					return {
						healthy: false,
						installation: state._tag,
						integrations: [],
					};
				}
				const integration_states = yield* integrations.Inspect(state.manifest.integrations);
				return {
					healthy: integration_states.every((item) => item._tag === "Owned"),
					installation: state._tag,
					integrations: integration_states,
				};
			}).pipe(Effect.mapError(MapFailure("doctor")));

		return DistributionOperations.of({
			Doctor,
			Repair: () =>
				Effect.gen(function* () {
					const state = yield* store.Inspect();
					const manifest = yield* RequireActive(state, "repair");
					const specifications = yield* integration_plan.Specifications(manifest);
					const repaired = yield* integrations.Repair(
						specifications,
						manifest.integrations,
					);
					yield* store.WriteAtomic({ ...manifest, integrations: repaired });
					return yield* Doctor();
				}).pipe(Effect.mapError(MapFailure("repair"))),
			Uninstall: (remove_data) =>
				Effect.gen(function* () {
					const state = yield* store.Inspect();
					const manifest = yield* RequireActive(state, "uninstall");
					const removed_integrations = yield* integrations.Uninstall(
						manifest.integrations,
					);
					if (remove_data) yield* removal.RemoveForgeData();
					const product_removal = yield* removal.RemoveProduct(manifest);
					return {
						forge_data_removed: remove_data,
						integrations: removed_integrations,
						manual_cleanup_command: product_removal.manual_cleanup_command,
					};
				}).pipe(Effect.mapError(MapFailure("uninstall"))),
			Update: () => installer.Converge().pipe(Effect.mapError(MapFailure("update"))),
		});
	}),
);

/** Used until a release/platform composition supplies the permanent installer adapters. */
export const DistributionOperationsUnavailable = Layer.succeed(
	DistributionOperations,
	DistributionOperations.of({
		Doctor: () =>
			Effect.fail(
				new DistributionOperationsError({
					code: "operation_unavailable",
					operation: "doctor",
				}),
			),
		Repair: () =>
			Effect.fail(
				new DistributionOperationsError({
					code: "operation_unavailable",
					operation: "repair",
				}),
			),
		Uninstall: () =>
			Effect.fail(
				new DistributionOperationsError({
					code: "operation_unavailable",
					operation: "uninstall",
				}),
			),
		Update: () =>
			Effect.fail(
				new DistributionOperationsError({
					code: "operation_unavailable",
					operation: "update",
				}),
			),
	}),
);
