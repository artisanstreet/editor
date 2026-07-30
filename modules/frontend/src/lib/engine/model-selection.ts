import type { RuntimeCatalog } from "@artisan/protocol";
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

export const thinking_level_labels: Readonly<Record<ThinkingLevel, string>> = {
	high: "High",
	light: "Light",
	max: "Max",
	medium: "Medium",
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
