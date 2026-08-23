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

/** Provider-neutral names for the sparse permission scale shared by every harness. */
export const permission_level_labels = {
	restricted: "Read only",
	autonomous: "Auto",
	unrestricted: "Unrestricted",
} satisfies Readonly<Record<PermissionOption["id"], string>>;

export const PermissionLevelLabel = (option: PermissionOption) =>
	permission_level_labels[option.id];

/** Makes the three shared permission traits comparable without hiding native behavior. */
export const PermissionTraitSummary = (option: PermissionOption) => {
	const scope =
		option.edit_scope === "none"
			? "No writes"
			: option.edit_scope === "workspace"
				? "Workspace"
				: "Host";
	const approval =
		option.approval_behavior === "prompts"
			? "Prompts"
			: option.approval_behavior === "classifier"
				? "Auto review"
				: "No prompts";
	const boundary =
		option.safety_boundary === "plan"
			? "Plan boundary"
			: option.safety_boundary === "sandbox"
				? "Sandboxed"
				: option.safety_boundary === "rules"
					? "Rules enforced"
					: "Ordinary checks bypassed";
	return `${scope} · ${approval} · ${boundary}`;
};

export const PermissionOptionDescription = (option: PermissionOption) =>
	`${PermissionTraitSummary(option)}. ${option.description}`;

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

export interface ModelRouteGroup {
	readonly id: string;
	readonly label: string;
	readonly models: ReadonlyArray<ModelChoice>;
	readonly show_route_labels: boolean;
	readonly status: "available" | "unavailable";
	readonly unavailable_reason?: string;
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
				permission: "autonomous",
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

const variant_family_key = (model: ModelChoice) => {
	const selection = model.definition.native_selection;
	return selection === undefined
		? model.id
		: JSON.stringify([model.engine, selection.provider_route_id, selection.model_id]);
};

/** All selectable variants belonging to one route/model row. */
export const VariantsForModel = (
	models: ReadonlyArray<ModelChoice>,
	model: ModelChoice,
): ReadonlyArray<ModelChoice> => {
	const key = variant_family_key(model);
	return models.filter((candidate) => variant_family_key(candidate) === key);
};

const native_variant_labels: Readonly<Record<string, string>> = {
	default: "Default",
	high: thinking_level_labels.high,
	light: thinking_level_labels.light,
	low: thinking_level_labels.light,
	max: thinking_level_labels.max,
	medium: thinking_level_labels.medium,
	minimal: "Minimal",
	ultra: thinking_level_labels.ultra,
	xhigh: thinking_level_labels.xhigh,
};

/** Presents provider-owned effort variants using Artisan's shared vocabulary. */
export const VariantLabel = (model: ModelChoice): string => {
	const variant_id = model.definition.native_selection?.variant_id;
	if (variant_id === undefined) return "Default";
	const normalized = variant_id.trim().toLowerCase();
	return (
		native_variant_labels[normalized] ??
		normalized
			.split(/[-_.\s]+/u)
			.filter((part) => part.length > 0)
			.map((part) => `${part[0]?.toUpperCase() ?? ""}${part.slice(1)}`)
			.join(" ")
	);
};

/** Formats a fixed provider-reported limit as metadata, not as a policy value. */
export const FormatContextWindowTokens = (tokens: number): string => {
	if (tokens >= 1_000_000) {
		const millions = tokens / 1_000_000;
		return `${Number.isInteger(millions) ? millions : millions.toFixed(1)}M context`;
	}
	if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K context`;
	return `${tokens.toLocaleString()} context`;
};

/** Groups route-aware model rows using adapter-owned live route metadata. */
export const RouteGroupsForModels = (
	catalog: RuntimeCatalog,
	engine: HarnessId,
	models: ReadonlyArray<ModelChoice>,
): ReadonlyArray<ModelRouteGroup> => {
	const routes = (catalog.routes ?? []).filter((route) => route.engine_id === engine);
	if (routes.length === 0) return [];
	const route_by_id = new Map(routes.map((route) => [route.id, route]));
	const groups = new Map<
		string,
		{
			label: string;
			models: Array<ModelChoice>;
			order: number;
			route_ids: Set<string>;
			show_route_labels: boolean;
			status: "available" | "unavailable";
			unavailable_reason?: string;
		}
	>();

	for (const route of routes) {
		const key = `${engine}:${route.group.id}`;
		const group = groups.get(key);
		if (group === undefined) {
			groups.set(key, {
				label: route.group.label,
				models: [],
				order: route.group.order,
				route_ids: new Set([route.id]),
				show_route_labels: route.group.show_route_labels,
				status: route.status,
				...(route.unavailable_reason === undefined
					? {}
					: { unavailable_reason: route.unavailable_reason }),
			});
			continue;
		}
		group.route_ids.add(route.id);
		if (route.group.show_route_labels) group.show_route_labels = true;
		if (route.status === "available") group.status = "available";
		if (group.unavailable_reason === undefined && route.unavailable_reason !== undefined)
			group.unavailable_reason = route.unavailable_reason;
	}

	for (const model of models) {
		const route_id = model.definition.native_selection?.provider_route_id;
		if (route_id === undefined) continue;
		const route = route_by_id.get(route_id);
		if (route === undefined) continue;
		groups.get(`${engine}:${route.group.id}`)?.models.push(model);
	}

	return [...groups.entries()]
		.sort(
			([, left], [, right]) =>
				left.order - right.order || left.label.localeCompare(right.label),
		)
		.map(([id, group]) => ({
			id,
			label: group.label,
			models: group.models,
			show_route_labels: group.show_route_labels || group.route_ids.size > 1,
			status: group.status,
			...(group.unavailable_reason === undefined
				? {}
				: { unavailable_reason: group.unavailable_reason }),
		}));
};

export const OrderModels = (
	models: ReadonlyArray<ModelChoice>,
	engine: HarnessId,
	favorites: ReadonlyArray<string>,
	selected_model_id?: string,
) => {
	const candidates = models.filter((model) => model.engine === engine);
	const favorite_rank = new Map(favorites.map((id, index) => [id, index]));
	const families = new Map<string, [ModelChoice, ...Array<ModelChoice>]>();
	for (const model of candidates) {
		const key = variant_family_key(model);
		const family = families.get(key);
		if (family === undefined) families.set(key, [model]);
		else family.push(model);
	}

	const rows = [...families.values()].map((family, index) => {
		const selected = family.find((model) => model.id === selected_model_id);
		let favorite: ModelChoice | undefined;
		let favorite_index = Number.POSITIVE_INFINITY;
		for (const model of family) {
			const rank = favorite_rank.get(model.id);
			if (rank !== undefined && rank < favorite_index) {
				favorite = model;
				favorite_index = rank;
			}
		}
		const base = family.find(
			(model) => model.definition.native_selection?.variant_id === undefined,
		);
		return {
			favorite_index,
			index,
			model: selected ?? favorite ?? base ?? family[0],
		};
	});

	return rows
		.sort((left, right) =>
			left.favorite_index === right.favorite_index
				? left.index - right.index
				: left.favorite_index - right.favorite_index,
		)
		.map((row) => row.model);
};
