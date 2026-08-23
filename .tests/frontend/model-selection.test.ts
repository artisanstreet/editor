import { describe, expect, it } from "vitest";

import { model_manifest } from "@artisan/catalog";
import type { RuntimeCatalog, ThreadSessionPolicy } from "@artisan/protocol";
import {
	permission_for_harness,
	permission_policy_for_harness,
	permission_policy_matches,
	permission_reconciliation_for_harness,
	policy_fields_for_permission,
	FormatContextWindowTokens,
	ModelsFromCatalog,
	OrderModels,
	PermissionLevelLabel,
	PermissionOptionDescription,
	PermissionTraitSummary,
	RouteGroupsForModels,
	VariantLabel,
	VariantsForModel,
} from "../../modules/frontend/src/lib/engine/model-selection";
import { ApplyPolicyPatch } from "../../modules/frontend/src/routes/components/model-selector/presentation";

const catalog = {
	manifest: model_manifest,
	runnable_harness_ids: ["codex", "claude"],
} satisfies RuntimeCatalog;

describe("model permission selection", () => {
	it("presents every native permission on one shared trait scale", () => {
		const options = catalog.manifest.harnesses.flatMap(
			(harness) => harness.permissions.options,
		);
		expect(new Set(options.map(PermissionLevelLabel))).toEqual(
			new Set(["Read only", "Auto", "Unrestricted"]),
		);
		const codex_auto = options.find(
			(option) => option.id === "autonomous" && option.native_value === "workspace-write",
		)!;
		const claude_auto = options.find(
			(option) => option.id === "autonomous" && option.native_value === "auto",
		)!;
		expect(PermissionTraitSummary(codex_auto)).toBe("Workspace · Prompts · Sandboxed");
		expect(PermissionTraitSummary(claude_auto)).toBe("Host · Auto review · Rules enforced");
		expect(PermissionOptionDescription(claude_auto)).toContain("Anthropic's classifier");
	});

	it("coalesces rapid reasoning, speed, and model intents from the latest desired policy", () => {
		const initial: ThreadSessionPolicy = {
			engine_id: "codex",
			model: "gpt-5.6-codex",
			permission: "supervised",
			permission_mode: "on_request",
			reasoning_effort: "medium",
			sandbox_mode: "workspace_write",
			service_tier: "standard",
			strict_clarification: false,
			web_search_enabled: false,
		};
		const reasoning = ApplyPolicyPatch(initial, { reasoning_effort: "high" });
		const speed = ApplyPolicyPatch(reasoning, { service_tier: "priority" });
		const model = ApplyPolicyPatch(speed, { engine_id: "claude", model: "claude-opus-4-1" });

		expect(model).toMatchObject({
			engine_id: "claude",
			model: "claude-opus-4-1",
			reasoning_effort: "high",
			service_tier: "priority",
		});
	});
	it("restores Full access with its no-prompt compatibility axes", () => {
		const option = permission_for_harness(catalog, "codex", "unrestricted");

		expect(option?.native_value).toBe("danger-full-access");
		expect(policy_fields_for_permission(option)).toEqual({
			permission: "unrestricted",
			permission_mode: "never",
			sandbox_mode: "workspace_write",
		});
	});

	it("falls back when a saved permission is unsupported by the selected harness", () => {
		const resolved = permission_policy_for_harness(catalog, "codex", "trusted");

		expect(resolved.option?.id).toBe("autonomous");
		expect(resolved.fields).toEqual({
			permission: "autonomous",
			permission_mode: "on_request",
			sandbox_mode: "workspace_write",
		});
		expect(resolved.option?.id).toBe(resolved.fields.permission);
	});

	it("keeps read-only policy axes aligned with the catalog", () => {
		const option = permission_for_harness(catalog, "codex", "restricted");

		expect(policy_fields_for_permission(option)).toEqual({
			permission: "restricted",
			permission_mode: "never",
			sandbox_mode: "read_only",
		});
	});

	it("repairs stale compatibility axes even when the permission id is valid", () => {
		const stale_policy: ThreadSessionPolicy = {
			engine_id: "codex",
			model: "gpt-5.6-codex",
			permission: "unrestricted",
			permission_mode: "on_request",
			reasoning_effort: "medium",
			sandbox_mode: "workspace_write",
			service_tier: "standard",
			strict_clarification: false,
			web_search_enabled: false,
		};
		const resolved = permission_reconciliation_for_harness(catalog, "codex", stale_policy);

		expect(resolved.needs_update).toBe(true);
		const reconciled = { ...stale_policy, ...resolved.fields };
		expect(permission_policy_matches(reconciled, resolved.fields)).toBe(true);
		expect(reconciled.permission_mode).toBe("never");
	});
});

describe("route-aware model variants", () => {
	it("uses the canonical lab while retaining a Hermes execution route", () => {
		const template = model_manifest.models.find(
			(model) => model.native_model_id === "gpt-5.4-mini" && model.harness === "codex",
		)!;
		const runtime_catalog = {
			manifest: {
				...model_manifest,
				models: [
					{
						...template,
						harness: "hermes" as const,
						id: "hermes-openai-gpt-5.4-mini",
						name: "gpt-5.4-mini",
						native_model_id: "openai/gpt-5.4-mini",
						native_selection: {
							model_id: "openai/gpt-5.4-mini",
							provider_route_id: "openai-codex",
						},
						provider: "openai",
						routing: {
							kind: "provider-route" as const,
							provider_route_id: "openai-codex",
						},
						status: "dynamic" as const,
					},
				],
				providers: [
					...model_manifest.providers,
					{ id: "openai-codex", label: "OpenAI Codex" },
				],
			},
			runnable_harness_ids: ["hermes"],
			routes: [
				{
					engine_id: "hermes",
					group: {
						id: "openai-codex",
						label: "OpenAI Codex",
						order: 0,
						show_route_labels: false,
					},
					id: "openai-codex",
					label: "OpenAI Codex",
					status: "available",
				},
			],
		} satisfies RuntimeCatalog;

		expect(ModelsFromCatalog(runtime_catalog)[0]).toMatchObject({
			lab: "OpenAI",
			name: "gpt-5.4-mini",
		});
	});

	it("renders Hermes providers as independent collapsible groups", () => {
		const template = model_manifest.models[0]!;
		const hermes_model = (provider_route_id: "nous" | "openrouter") => ({
			...template,
			harness: "hermes" as const,
			id: `hermes-${provider_route_id}-sonnet`,
			name: "Claude Sonnet 4.6",
			native_model_id: "anthropic/claude-sonnet-4.6",
			native_selection: {
				model_id: "anthropic/claude-sonnet-4.6",
				provider_route_id,
			},
			provider: provider_route_id,
			routing: { kind: "provider-route" as const, provider_route_id },
			status: "dynamic" as const,
		});
		const runtime_catalog = {
			manifest: {
				...model_manifest,
				models: [hermes_model("nous"), hermes_model("openrouter")],
				providers: [
					...model_manifest.providers,
					{ id: "nous", label: "Nous Portal" },
					{ id: "openrouter", label: "OpenRouter" },
				],
			},
			runnable_harness_ids: ["hermes"],
			routes: [
				{
					engine_id: "hermes",
					group: { id: "nous", label: "Nous Portal", order: 0, show_route_labels: false },
					id: "nous",
					label: "Nous Portal",
					status: "available",
				},
				{
					engine_id: "hermes",
					group: {
						id: "openrouter",
						label: "OpenRouter",
						order: 1,
						show_route_labels: false,
					},
					id: "openrouter",
					label: "OpenRouter",
					status: "available",
				},
			],
		} satisfies RuntimeCatalog;
		const models = ModelsFromCatalog(runtime_catalog);
		const groups = RouteGroupsForModels(
			runtime_catalog,
			"hermes",
			OrderModels(models, "hermes", []),
		);

		expect(groups.map((group) => group.label)).toEqual(["Nous Portal", "OpenRouter"]);
		expect(groups.every((group) => group.models.length === 1)).toBe(true);
	});

	it("presents provider variants as one model row and keeps routes distinct", () => {
		const template = model_manifest.models[0]!;
		const dynamic_model = (
			id: string,
			provider_route_id: "opencode" | "opencode-go" | "acme",
			variant_id?: string,
		) => ({
			...template,
			harness: "opencode2" as const,
			id,
			name: "Ox Alpha Free (Unlimited)",
			native_model_id: "ox-alpha-free",
			native_selection: {
				model_id: "ox-alpha-free",
				provider_route_id,
				...(variant_id === undefined ? {} : { variant_id }),
			},
			provider: provider_route_id === "acme" ? "acme" : "unknown",
			routing: { kind: "provider-route" as const, provider_route_id },
			status: "dynamic" as const,
		});
		const runtime_catalog = {
			manifest: {
				...model_manifest,
				models: [
					dynamic_model("zen-default", "opencode"),
					dynamic_model("zen-minimal", "opencode", "minimal"),
					dynamic_model("zen-low", "opencode", "low"),
					dynamic_model("zen-medium", "opencode", "medium"),
					dynamic_model("zen-high", "opencode", "high"),
					dynamic_model("zen-xhigh", "opencode", "xhigh"),
					dynamic_model("zen-max", "opencode", "max"),
					dynamic_model("go-default", "opencode-go"),
					dynamic_model("acme-default", "acme"),
				],
				providers: [...model_manifest.providers, { id: "acme", label: "Acme" }],
			},
			runnable_harness_ids: ["opencode2"],
			routes: [
				{
					engine_id: "opencode2",
					group: { id: "go", label: "Go", order: 0, show_route_labels: false },
					id: "opencode-go",
					label: "Go",
					status: "available",
				},
				{
					engine_id: "opencode2",
					group: { id: "zen", label: "Zen", order: 1, show_route_labels: false },
					id: "opencode",
					label: "Zen",
					status: "available",
				},
				{
					engine_id: "opencode2",
					group: {
						id: "custom",
						label: "Custom",
						order: 2,
						show_route_labels: true,
					},
					id: "acme",
					label: "Acme",
					status: "available",
				},
			],
		} satisfies RuntimeCatalog;
		const models = ModelsFromCatalog(runtime_catalog);

		expect(OrderModels(models, "opencode2", []).map((model) => model.id)).toEqual([
			"zen-default",
			"go-default",
			"acme-default",
		]);
		expect(OrderModels(models, "opencode2", [], "zen-high")[0]?.id).toBe("zen-high");
		expect(VariantsForModel(models, models[0]!).map((model) => model.id)).toEqual([
			"zen-default",
			"zen-minimal",
			"zen-low",
			"zen-medium",
			"zen-high",
			"zen-xhigh",
			"zen-max",
		]);
		expect(VariantsForModel(models, models[0]!).map(VariantLabel)).toEqual([
			"Default",
			"Minimal",
			"Light",
			"Medium",
			"High",
			"Extra High",
			"Max",
		]);
		const groups = RouteGroupsForModels(
			runtime_catalog,
			"opencode2",
			OrderModels(models, "opencode2", []),
		);
		expect(groups.map((group) => group.label)).toEqual(["Go", "Zen", "Custom"]);
		expect(groups[0]?.models[0]).toMatchObject({ id: "go-default", lab: "Unknown" });
		expect(groups[1]?.models[0]).toMatchObject({ id: "zen-default", lab: "Unknown" });
		expect(groups[2]).toMatchObject({
			models: [expect.objectContaining({ id: "acme-default", lab: "Acme" })],
			show_route_labels: true,
		});
	});

	it("formats reported context as metadata rather than a selectable value", () => {
		expect(FormatContextWindowTokens(200_000)).toBe("200K context");
		expect(FormatContextWindowTokens(1_000_000)).toBe("1M context");
		expect(FormatContextWindowTokens(1_050_000)).toBe("1.1M context");
	});
});
