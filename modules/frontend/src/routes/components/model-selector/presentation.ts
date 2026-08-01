import type { SessionDefaults } from "@artisan/protocol";
import type {
	ContextWindowChoice,
	ModelChoice,
	PermissionOption,
	ThinkingLevel,
} from "../../../lib/engine/model-selection";
import type { ThreadSessionPolicy } from "@artisan/protocol";

/** Applies the newest intent to the latest desired policy without dropping prior axes. */
export const ApplyPolicyPatch = (
	policy: ThreadSessionPolicy,
	patch: Partial<ThreadSessionPolicy>,
): ThreadSessionPolicy => ({ ...policy, ...patch });

/** Shared catalog presentation for durable model defaults and session policies. */
export const ModelsForEngine = (
	models: ReadonlyArray<ModelChoice>,
	engine: ModelChoice["engine"],
): ReadonlyArray<ModelChoice> => models.filter((model) => model.engine === engine);

export const PermissionsForSelection = (
	model: ModelChoice,
	permissions: ReadonlyArray<PermissionOption>,
): ReadonlyArray<PermissionOption> =>
	permissions.filter(() => model.definition.disabled === undefined);

export const ThinkingForDefaults = (
	defaults: SessionDefaults,
	model: ModelChoice,
): ThinkingLevel | undefined => {
	const capability = model.definition.capabilities.thinking;
	if (capability.availability !== "supported") return undefined;
	const saved = defaults.models.find((entry) => entry.model_id === model.id)?.reasoning_effort;
	return saved === "low" ? "light" : (saved ?? capability.default);
};

export const ContextForDefaults = (
	defaults: SessionDefaults,
	model: ModelChoice,
): ContextWindowChoice | undefined => {
	const context = model.definition.capabilities.context_window;
	if (context === undefined) return undefined;
	const saved = defaults.models.find((entry) => entry.model_id === model.id)?.context_window;
	return (
		context.options.find((option) => option.native_suffix === (saved ?? "")) ??
		context.options.find((option) => option.id === context.default) ??
		context.options[0]
	);
};
