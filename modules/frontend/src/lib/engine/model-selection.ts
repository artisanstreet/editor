import {
	SessionPolicyPermission,
	type RuntimeCatalog,
	type ThreadSessionPolicy,
} from "@artisan/protocol";
import type { Component } from "svelte";

export type ModelDefinition = RuntimeCatalog["manifest"]["models"][number];
export type HarnessId = ModelDefinition["harness"];
export type PermissionOption =
	RuntimeCatalog["manifest"]["harnesses"][number]["permissions"]["options"][number];
export type SpeedOption = ModelDefinition["capabilities"]["speed_options"][number];
export type ThinkingLevel = Exclude<
	ModelDefinition["capabilities"]["thinking"],
	{ readonly availability: "native" | "unavailable" }
>["options"][number]["id"];
export type ContextWindowChoice = NonNullable<
	ModelDefinition["capabilities"]["context_window"]
>["options"][number];

export interface EngineChoice {
	readonly id: HarnessId;
	readonly name: string;
	readonly icon: Component;
	readonly monochrome: boolean;
}

export interface ModelChoice {
	readonly definition: ModelDefinition;
	readonly id: string;
	readonly engine: HarnessId;
	readonly name: string;
	readonly lab: string;
}

export const PermissionsForModel = (catalog: RuntimeCatalog, model: ModelChoice) =>
	catalog.manifest.harnesses.find((harness) => harness.id === model.engine)?.permissions;

/** Resolves one harness-supported choice, falling back to its curated default. */
export const permission_for_harness = (
	catalog: RuntimeCatalog,
	engine: HarnessId,
	preferred_id: string,
): PermissionOption | undefined => {
	const permissions = catalog.manifest.harnesses.find(
		(harness) => harness.id === engine,
	)?.permissions;

	return (
		permissions?.options.find((option) => option.id === preferred_id) ??
		permissions?.options.find((option) => option.id === permissions.default) ??
		permissions?.options[0]
	);
};

/** Projects the authoritative catalog option onto the policy's compatibility axes. */
export const policy_fields_for_permission = (
	option: PermissionOption | undefined,
): Pick<ThreadSessionPolicy, "permission" | "permission_mode" | "sandbox_mode"> =>
	option === undefined
		? {
				permission: "supervised",
				permission_mode: "on_request",
				sandbox_mode: "workspace_write",
			}
		: {
				permission: option.id,
				permission_mode: option.approval_behavior === "none" ? "never" : "on_request",
				sandbox_mode: option.edit_scope === "none" ? "read_only" : "workspace_write",
			};

/** Resolves the displayed option and emitted policy fields as one invariant. */
export const permission_policy_for_harness = (
	catalog: RuntimeCatalog,
	engine: HarnessId,
	preferred_id: string,
) => {
	const option = permission_for_harness(catalog, engine, preferred_id);
	return { fields: policy_fields_for_permission(option), option } as const;
};

/** Whether the explicit permission and both compatibility axes agree. */
export const permission_policy_matches = (
	policy: ThreadSessionPolicy,
	fields: Pick<ThreadSessionPolicy, "permission" | "permission_mode" | "sandbox_mode">,
): boolean =>
	SessionPolicyPermission(policy) === fields.permission &&
	policy.permission_mode === fields.permission_mode &&
	policy.sandbox_mode === fields.sandbox_mode;

/** Resolves a policy against one harness and reports whether it must be repaired. */
export const permission_reconciliation_for_harness = (
	catalog: RuntimeCatalog,
	engine: HarnessId,
	policy: ThreadSessionPolicy,
) => {
	const resolved = permission_policy_for_harness(
		catalog,
		engine,
		SessionPolicyPermission(policy),
	);
	return {
		...resolved,
		needs_update: !permission_policy_matches(policy, resolved.fields),
	} as const;
};

export const thinking_level_labels: Readonly<Record<ThinkingLevel, string>> = {
	high: "High",
	light: "Light",
	max: "Max",
	medium: "Medium",
	ultra: "Ultra",
	xhigh: "Extra High",
};

export const ModelsFromCatalog = (catalog: RuntimeCatalog): ReadonlyArray<ModelChoice> => {
	const labels = new Map(
		catalog.manifest.providers.map((provider) => [provider.id, provider.label]),
	);
	return catalog.manifest.models.map((model) => ({
		definition: model,
		engine: model.harness,
		id: model.id,
		lab: labels.get(model.provider) ?? model.provider,
		name: model.name,
	}));
};

export const OrderModels = (
	models: ReadonlyArray<ModelChoice>,
	engine: HarnessId,
	favorites: ReadonlyArray<string>,
) => {
	const candidates = models.filter((model) => model.engine === engine);
	const starred = candidates.filter((model) => favorites.includes(model.id));
	return starred.length === 0
		? candidates
		: [
				...[...starred].sort(
					(left, right) => favorites.indexOf(left.id) - favorites.indexOf(right.id),
				),
				...candidates.filter((model) => !favorites.includes(model.id)),
			];
};
