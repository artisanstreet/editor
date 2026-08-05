import type { RuntimeCatalog, SurfaceUsageAggregate } from "@artisan/protocol";

/**
 * Resolves the model that actually reported the context gauge. A thread policy
 * selects a future launch and must never relabel telemetry from an earlier run.
 */
export const ContextUsageModelName = (
	usage: SurfaceUsageAggregate,
	runtime_catalog: RuntimeCatalog,
): string | undefined => {
	const origin = usage.context_origin;
	if (origin?.engine_id === undefined || origin.model_id === undefined) return undefined;

	return runtime_catalog.manifest.models.find(
		(model) => model.harness === origin.engine_id && model.native_model_id === origin.model_id,
	)?.name;
};
