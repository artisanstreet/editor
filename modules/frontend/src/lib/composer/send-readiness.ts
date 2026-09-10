import type { RuntimeCatalog, SurfaceUsageAggregate, ThreadSessionPolicy } from "@artisan/protocol";

/**
 * Renderer-observed provisioning for one engine. This is the installer's
 * answer, not the catalog's: whether Artisan's managed binary is present,
 * whether a failure or an in-flight install was observed, and whether a
 * previous version proves a binary went missing rather than never existing.
 */
export interface EngineProvisioning {
	/** Whether Artisan's managed binary is installed and active. */
	readonly managed: boolean;
	/** Active managed version, when one is installed. */
	readonly active_version?: string;
	/** Previously active version, when a binary went missing. */
	readonly previous_version?: string;
	/** User-facing install failure, when the last attempt failed. */
	readonly failure?: string;
	/** Whether an install or sign-in is still running for this engine. */
	readonly busy: boolean;
}

/**
 * Sending needs a live engine behind it. Without Forge there is no session to
 * run at all, and within a connected catalog a model whose harness is
 * unregistered can still be picked and read but never run.
 *
 * The runnable set stays the authority for ready: nothing below returns ready
 * for an unrunnable engine. The live catalog (model and route availability)
 * and the renderer's own provisioning observation only ever add honest
 * not-ready reasons — a missing binary still blocks with its reason even
 * when the catalog lists the engine as runnable.
 */
export const ComposerSendBlockedReason = (
	forge_available: boolean,
	catalog: RuntimeCatalog,
	policy: ThreadSessionPolicy | undefined,
	provisioning?: EngineProvisioning | undefined,
): string | undefined => {
	if (!forge_available) return "Forge is offline — reconnect to send";
	const engine_id = policy?.engine_id;
	if (engine_id === undefined || policy === undefined) return undefined;
	const label =
		catalog.manifest.harnesses.find((harness) => harness.id === engine_id)?.label ?? engine_id;
	const definition = catalog.manifest.models.find((model) => {
		if (model.harness !== engine_id) return false;
		if (model.id === policy.model || model.native_model_id === policy.model) return true;
		const selection = model.native_selection;
		return (
			selection !== undefined && selection.model_id === (policy.model_id ?? policy.model)
		);
	});
	if (definition?.disabled !== undefined) return definition.disabled.reason;
	const routes = (catalog.routes ?? []).filter((route) => route.engine_id === engine_id);
	if (routes.length > 0 && routes.every((route) => route.status === "unavailable")) {
		return (
			routes
				.map((route) => route.unavailable_reason)
				.find((reason) => reason !== undefined) ??
			`${label} is unavailable on this Forge right now`
		);
	}
	if (
		provisioning !== undefined &&
		!provisioning.managed &&
		provisioning.active_version === undefined
	) {
		if (provisioning.failure !== undefined)
			return `${label} could not start — ${provisioning.failure}`;
		if (provisioning.busy) return `${label} is still installing — try again when it finishes`;
		if (provisioning.previous_version !== undefined)
			return `${label}'s installed binary is missing — repair it to send`;
		return `${label} is not set up on this machine yet — install it to send`;
	}
	if (catalog.runnable_harness_ids.includes(engine_id)) return undefined;
	return `${label} models are preview-only — this engine cannot run in Artisan yet`;
};

/**
 * Reports whether a context reading describes what the thread would launch now.
 *
 * A reading is telemetry from one immutable run, and a thread outlives the
 * engine and model that produced it. Nothing scopes the stored gauge to its
 * reporter — the surface projection carries the newest non-null window forward
 * across every run in the thread — so a Codex run's reported window survived
 * onto Claude threads, where the gauge read 252K beside a picker saying 1M.
 *
 * A run whose model was never recorded still matches on its engine alone.
 * Requiring the model would drop the gauge for every such run, which is a
 * wider silence than the mismatch it guards against.
 */
export const ComposerContextUsageIsCurrent = (
	policy: ThreadSessionPolicy | undefined,
	context_usage: SurfaceUsageAggregate | undefined,
): boolean => {
	const origin = context_usage?.context_origin;
	if (origin === undefined || policy === undefined) return false;
	if (origin.engine_id !== policy.engine_id) return false;
	return origin.model_id === undefined || origin.model_id === policy.model;
};

/**
 * The context-window denominator. A provider that discloses its usable window
 * on the wire (Codex) wins for as long as the thread is still on the run that
 * disclosed it; otherwise the catalog's configured context-window option for
 * the thread's model stands in (Claude never reports a window size). Models
 * without the capability show no gauge.
 *
 * A policy without a context-window choice resolves to the capability's
 * default option, never the suffix-less one. The launcher composes the model
 * id as `model + (suffix ?? "")`, and the harness resolves a bare Claude 5 id
 * to its extended window — live sessions measured 236K+ tokens of context on
 * plain model ids. Dividing those readings by the 200K "standard" option is
 * what pinned the gauge at 100% for turns on end.
 */
export const ComposerContextWindowTokens = (
	catalog: RuntimeCatalog,
	policy: ThreadSessionPolicy | undefined,
	context_usage: SurfaceUsageAggregate | undefined,
): number | undefined => {
	if (
		context_usage?.context_window_tokens !== undefined &&
		ComposerContextUsageIsCurrent(policy, context_usage)
	) {
		return context_usage.context_window_tokens;
	}
	const capabilities = catalog.manifest.models.find((model) => {
		if (model.harness !== policy?.engine_id || model.native_model_id !== policy.model)
			return false;
		const selection = model.native_selection;
		return selection === undefined
			? policy.provider_route_id === undefined && policy.variant_id === undefined
			: selection.provider_route_id === policy.provider_route_id &&
					selection.model_id === (policy.model_id ?? policy.model) &&
					selection.variant_id === policy.variant_id;
	})?.capabilities;
	const capability = capabilities?.context_window;
	if (capability === undefined) return capabilities?.context_window_tokens;
	const default_option = capability.options.find(
		(candidate) => candidate.id === capability.default,
	);
	const option =
		policy?.context_window === undefined
			? default_option
			: (capability.options.find(
					(candidate) => candidate.native_suffix === policy.context_window,
				) ?? default_option);
	return option?.tokens;
};
