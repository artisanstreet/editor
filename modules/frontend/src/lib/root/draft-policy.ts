import type { RuntimeCatalog, SessionDefaults, ThreadSessionPolicy } from "@artisan/protocol";

import { permission_for_harness, policy_fields_for_permission } from "../engine/model-selection";

/**
 * The session policy a brand-new draft opens with.
 *
 * The draft is the only place the engine can be chosen — it locks once the
 * first message creates the session — so this is the one moment the choice is
 * made for the reader. A new draft inherits the model they chose last time
 * (with that model's own default effort and speed, as if re-picked); with no
 * usable history it mirrors the backend's default session policy with the
 * runtime catalog's default model engine.
 *
 * Pure, and given the defaults rather than reading them, because it is the
 * rule and not the fetch: which model, which permission, and which per-model
 * effort and speed survive a restart is worth stating in one place that both a
 * route and a test can reach.
 */
export const SeededDraftPolicy = (
	runtime_catalog: RuntimeCatalog,
	defaults: SessionDefaults,
): ThreadSessionPolicy => {
	/**
	 * Catalog ids distinguish the same provider model exposed through different
	 * harnesses. The native-id lookup remains only as a compatibility path for
	 * defaults written before the catalog id became authoritative.
	 */
	const available_models = runtime_catalog.manifest.models.filter(
		(model) => model.disabled === undefined,
	);
	const last_model =
		available_models.find((model) => model.id === defaults.last_model_id) ??
		available_models.find((model) => model.native_model_id === defaults.last_model_id);
	const default_model = runtime_catalog.manifest.models.find(
		(model) => model.id === runtime_catalog.default_model_id,
	);
	const seed_model = last_model ?? default_model;
	const seed_thinking = seed_model?.capabilities.thinking;
	/** Effort, speed, and context are per model: only the chosen model's row applies. */
	const model_defaults = defaults.models.find((entry) => entry.model_id === seed_model?.id);
	const speed_options = seed_model?.capabilities.speed_options ?? [];
	/** A retired or currently unavailable tier returns to the model's own default. */
	const saved_speed = speed_options.find(
		(option) =>
			option.native_value === model_defaults?.service_tier && option.disabled === undefined,
	);
	const default_speed =
		speed_options.find((option) => option.default && option.disabled === undefined) ??
		speed_options.find((option) => option.disabled === undefined);
	/** A saved option from another harness falls back to this harness's default. */
	const seed_permission = permission_for_harness(
		runtime_catalog,
		seed_model?.harness ?? "codex",
		defaults.permission,
	);

	return {
		engine_id: seed_model?.harness ?? "codex",
		...(model_defaults?.context_window === undefined
			? {}
			: { context_window: model_defaults.context_window }),
		model: seed_model?.native_model_id,
		...policy_fields_for_permission(seed_permission),
		reasoning_effort:
			model_defaults?.reasoning_effort ??
			(seed_thinking?.availability === "supported"
				? seed_thinking.default === "light"
					? "low"
					: seed_thinking.default
				: "medium"),
		service_tier: saved_speed?.native_value ?? default_speed?.native_value ?? "standard",
		strict_clarification: false,
		web_search_enabled: false,
	} satisfies ThreadSessionPolicy;
};
