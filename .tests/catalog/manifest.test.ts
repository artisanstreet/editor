import {
	ModelDefinition,
	ModelManifest,
	PermissionCapability,
	SpeedOptions,
	SupportedThinkingCapability,
	model_manifest,
	thinking_level_labels,
} from "@artisan/catalog";
import { Schema } from "effect";
import { describe, expect, it } from "vitest";

describe("model catalog", () => {
	it("decodes the complete curated manifest", () => {
		expect(Schema.decodeUnknownSync(ModelManifest)(model_manifest)).toEqual(model_manifest);
	});

	it("uses xhigh and ultra as canonical IDs with product labels", () => {
		expect(thinking_level_labels.xhigh).toBe("Extra High");
		expect(thinking_level_labels.ultra).toBe("Ultra");
		expect("extra-high" in thinking_level_labels).toBe(false);
	});

	it("maps provider-native low-equivalent values into unified Light", () => {
		const supported = model_manifest.models
			.map((model) => model.capabilities.thinking)
			.filter((thinking) => thinking.availability === "supported");

		for (const thinking of supported) {
			const light = thinking.options.find((option) => option.id === "light");
			expect(["low", "minimal", "none"]).toContain(light?.native_value);
		}
	});

	it("groups base and special effort options in the catalog", () => {
		for (const model of model_manifest.models) {
			if (model.capabilities.thinking.availability !== "supported") {
				continue;
			}
			for (const option of model.capabilities.thinking.options) {
				expect(option.presentation_group).toBe(
					option.id === "max" || option.id === "ultra" ? "special" : "base",
				);
			}
		}
	});

	it("exposes Max and Ultra for direct GPT 5.6 Codex models only", () => {
		type PresentedModelOptions = [string, Array<[string, string]>];
		const special_options = (
			harness: "codex" | "claude" | "grok" | "cursor",
		): Array<PresentedModelOptions> =>
			model_manifest.models.flatMap((model): Array<PresentedModelOptions> => {
				const thinking = model.capabilities.thinking;
				if (model.harness !== harness || thinking.availability !== "supported") {
					return [];
				}
				return [
					[
						model.id,
						thinking.options
							.filter((option) => option.presentation_group === "special")
							.map((option): [string, string] => [option.id, option.native_value]),
					],
				];
			});

		expect(special_options("codex")).toEqual([
			[
				"codex-sol",
				[
					["max", "max"],
					["ultra", "ultra"],
				],
			],
			[
				"codex-terra",
				[
					["max", "max"],
					["ultra", "ultra"],
				],
			],
			[
				"codex-luna",
				[
					["max", "max"],
					["ultra", "ultra"],
				],
			],
			["codex-gpt-5-5", []],
			["codex-gpt-5-4", []],
			["codex-gpt-5-4-mini", []],
			["codex-spark", []],
		]);
		expect(special_options("claude")).toEqual([
			["claude-fable", [["max", "max"]]],
			["claude-opus", [["max", "max"]]],
			["claude-sonnet", [["max", "max"]]],
		]);
		expect(special_options("grok")).toEqual([
			["grok-4-6", []],
			["grok-4-5", []],
		]);
		expect(special_options("cursor").filter(([, options]) => options.length > 0)).toEqual([
			["cursor-claude-fable-5", [["max", "max"]]],
			["cursor-claude-opus-5", [["max", "max"]]],
			["cursor-claude-sonnet-5", [["max", "max"]]],
			["cursor-kimi-k3", [["max", "max"]]],
			["cursor-glm-5-2", [["max", "max"]]],
		]);
		expect(
			special_options("cursor").find(([model_id]) => model_id === "cursor-gpt-5-6-sol"),
		).toEqual(["cursor-gpt-5-6-sol", []]);

		const sol_thinking = model_manifest.models.find((model) => model.id === "codex-sol")
			?.capabilities.thinking;
		if (sol_thinking?.availability !== "supported") {
			throw new Error("GPT 5.6 Sol must expose supported thinking options");
		}
		expect(
			sol_thinking.options.filter((option) => option.presentation_group === "special"),
		).toMatchObject([
			{ economics: "diminishing-returns", id: "max", presentation_group: "special" },
			{
				/**
				 * Ultra bills as many model runs rather than one, and how many is
				 * the harness's decision — so the picker has to say so before the
				 * choice, in the tone a cost warrants.
				 */
				advisory: "Not recommended.",
				economics: "harness-orchestration",
				id: "ultra",
				presentation_group: "special",
			},
		]);
		expect(sol_thinking.options.find((option) => option.id === "ultra")?.description).toContain(
			"Each subagent is a separate model run with its own context",
		);
	});

	it("lists every first-party model exposed by the supported coding harnesses", () => {
		const native_models_by_harness = (harness: "codex" | "claude" | "grok" | "cursor") =>
			model_manifest.models
				.filter((model) => model.harness === harness)
				.map((model) => model.native_model_id);

		expect(native_models_by_harness("codex")).toEqual([
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			"gpt-5.6-luna",
			"gpt-5.5",
			"gpt-5.4",
			"gpt-5.4-mini",
			"gpt-5.3-codex-spark",
		]);
		expect(native_models_by_harness("claude")).toEqual([
			"claude-fable-5",
			"claude-opus-5",
			"claude-sonnet-5",
			"claude-haiku-4-5",
		]);
		expect(native_models_by_harness("grok")).toEqual([
			"grok-4.6",
			"grok-4.5",
			"grok-4.3",
			"grok-build-0.1",
			"composer-2.5",
		]);
		expect(native_models_by_harness("cursor")).toEqual([
			"composer-2.5",
			"auto",
			"cursor-grok-4.6",
			"cursor-grok-4.5",
			"gpt-5.6-sol",
			"gpt-5.6-terra",
			"gpt-5.6-luna",
			"gpt-5.5",
			"gpt-5.4",
			"gpt-5.4-mini",
			"gpt-5.3-codex",
			"claude-fable-5",
			"claude-opus-5",
			"claude-sonnet-5",
			"claude-haiku-4-5",
			"gemini-3.7-flash",
			"gemini-3.1-pro",
			"kimi-k3",
			"glm-5.2",
		]);
	});

	it("rejects unsupported defaults and non-bottom-up options", () => {
		expect(() =>
			Schema.decodeUnknownSync(SupportedThinkingCapability)({
				availability: "supported",
				default: "max",
				options: [
					{
						economics: "standard",
						id: "high",
						native_value: "high",
						presentation_group: "base",
					},
					{
						economics: "standard",
						id: "light",
						native_value: "low",
						presentation_group: "base",
					},
				],
			}),
		).toThrow();
	});

	it("allows sparse native levels without inventing intermediate support", () => {
		const sparse = Schema.decodeUnknownSync(SupportedThinkingCapability)({
			availability: "supported",
			default: "high",
			options: [
				{
					economics: "standard",
					id: "light",
					native_value: "low",
					presentation_group: "base",
				},
				{
					economics: "standard",
					id: "high",
					native_value: "high",
					presentation_group: "base",
				},
			],
		});

		expect(sparse.options.map((option) => option.id)).toEqual(["light", "high"]);
	});

	it("contains the primary coding harnesses and providers", () => {
		expect(model_manifest.harnesses.map((harness) => harness.id)).toEqual([
			"codex",
			"claude",
			"opencode2",
			"hermes",
			"grok",
			"cursor",
		]);
		expect(model_manifest.providers.map((provider) => provider.id)).toEqual([
			"openai",
			"anthropic",
			"xai",
			"cursor",
			"google",
			"moonshot",
			"zai",
		]);
	});

	it("rejects ambiguous native values and invalid speed option sets", () => {
		expect(() =>
			Schema.decodeUnknownSync(SupportedThinkingCapability)({
				availability: "supported",
				default: "light",
				options: [
					{
						economics: "standard",
						id: "light",
						native_value: "low",
						presentation_group: "base",
					},
					{
						economics: "standard",
						id: "high",
						native_value: "low",
						presentation_group: "base",
					},
				],
			}),
		).toThrow();
		expect(() =>
			Schema.decodeUnknownSync(SpeedOptions)([
				{
					availability: "always",
					consumption_basis: "standard",
					consumption_multiplier: 1,
					default: true,
					description: "Ordinary route.",
					id: "standard",
					label: "Standard",
					native_value: "standard",
					source_url: "https://example.test/speed",
					speed_multiplier: 1,
					verified_at: "2026-07-27",
				},
				{
					availability: "always",
					consumption_basis: "standard",
					consumption_multiplier: 1,
					default: true,
					description: "Another route.",
					id: "other",
					label: "Other",
					native_value: "standard",
					source_url: "https://example.test/speed",
					speed_multiplier: 1,
					verified_at: "2026-07-27",
				},
			]),
		).toThrow();
	});

	it("models documented provider speed and consumption ratios", () => {
		const sol = model_manifest.models.find((model) => model.id === "codex-sol");
		const opus = model_manifest.models.find((model) => model.id === "claude-opus");
		const grok = model_manifest.models.find((model) => model.id === "grok-4-5");
		const grok_46 = model_manifest.models.find((model) => model.id === "grok-4-6");
		const composer = model_manifest.models.find((model) => model.id === "cursor-composer-2-5");
		const cursor_grok_46 = model_manifest.models.find(
			(model) => model.id === "cursor-grok-4-6",
		);
		const cursor_grok = model_manifest.models.find((model) => model.id === "cursor-grok-4-5");

		expect(sol?.capabilities.speed_options).toMatchObject([
			{ consumption_multiplier: 1, id: "standard", speed_multiplier: 1 },
			{
				availability: "dynamic",
				consumption_basis: "chatgpt-credits",
				consumption_multiplier: 2.5,
				id: "fast",
				speed_multiplier: 1.5,
			},
		]);
		expect(opus?.capabilities.speed_options).toMatchObject([
			{ consumption_multiplier: 1, id: "standard", speed_multiplier: 1 },
			{
				availability: "dynamic",
				consumption_basis: "usage-credit-price",
				consumption_multiplier: 2,
				id: "fast",
				speed_multiplier: 2.5,
			},
		]);
		expect(grok?.capabilities.speed_options).toMatchObject([
			{ consumption_multiplier: 1, id: "standard", speed_multiplier: 1 },
		]);
		expect(grok_46?.capabilities.thinking).toMatchObject({
			availability: "supported",
			default: "high",
			options: [
				{ id: "light", native_value: "low" },
				{ id: "medium", native_value: "medium" },
				{ id: "high", native_value: "high" },
				{ id: "xhigh", native_value: "xhigh" },
			],
		});
		expect(grok_46?.capabilities.speed_options).toMatchObject([
			{ consumption_multiplier: 1, id: "standard", speed_multiplier: 1 },
		]);
		expect(composer?.capabilities.speed_options).toMatchObject([
			{
				consumption_multiplier: 1,
				id: "standard",
				input_consumption_multiplier: 1,
				output_consumption_multiplier: 1,
				speed_multiplier: 1,
			},
			{
				availability: "dynamic",
				consumption_basis: "usage-credit-price",
				consumption_multiplier: 6,
				id: "fast",
				input_consumption_multiplier: 6,
				output_consumption_multiplier: 6,
				speed_multiplier: null,
			},
		]);
		expect(cursor_grok?.capabilities.speed_options).toMatchObject([
			{ id: "standard" },
			{
				consumption_multiplier: null,
				id: "fast",
				input_consumption_multiplier: 2,
				output_consumption_multiplier: 3,
				speed_multiplier: null,
			},
		]);
		expect(cursor_grok_46?.capabilities.speed_options).toMatchObject([
			{ id: "standard" },
			{
				consumption_multiplier: 2,
				id: "fast",
				input_consumption_multiplier: 2,
				output_consumption_multiplier: 2,
				speed_multiplier: null,
			},
		]);
		for (const model of model_manifest.models) {
			for (const option of model.capabilities.speed_options) {
				expect(option.description).toMatch(new RegExp(`^${model.name} uses `));
				expect(option.description).toMatch(/Fast mode is (available|not available)/);
			}
		}
	});

	it("represents disabled models by the presence of a reason only", () => {
		const composer = model_manifest.models.find((model) => model.id === "cursor-composer-2-5");
		expect(composer).toBeDefined();

		const disabled = Schema.decodeUnknownSync(ModelDefinition)({
			...composer,
			disabled: { reason: "Unavailable for this account." },
		});

		expect(disabled.disabled).toEqual({ reason: "Unavailable for this account." });
		expect("enabled" in disabled).toBe(false);
	});

	it("accepts arbitrary future speed tiers without changing the schema", () => {
		const options = Schema.decodeUnknownSync(SpeedOptions)([
			{
				availability: "always",
				consumption_basis: "standard",
				consumption_multiplier: 1,
				default: false,
				description: "Trades latency for lower cost.",
				id: "economy",
				label: "Economy",
				native_value: "provider-economy",
				source_url: "https://example.test/speed",
				speed_multiplier: 1,
				verified_at: "2026-07-27",
			},
			{
				availability: "always",
				consumption_basis: "standard",
				consumption_multiplier: 1,
				default: true,
				description: "The provider default.",
				id: "standard",
				label: "Standard",
				native_value: "provider-standard",
				source_url: "https://example.test/speed",
				speed_multiplier: 1,
				verified_at: "2026-07-27",
			},
			{
				availability: "dynamic",
				consumption_basis: "usage-credit-price",
				consumption_multiplier: 2,
				default: false,
				description: "Runs on a dedicated inference backend.",
				id: "accelerated",
				label: "Accelerated",
				native_value: "provider-accelerated",
				source_url: "https://example.test/speed",
				speed_multiplier: 2,
				verified_at: "2026-07-27",
			},
		]);

		expect(options.map((option) => option.id)).toEqual(["economy", "standard", "accelerated"]);
	});

	it("rejects contradictory aggregate and component speed economics", () => {
		expect(() =>
			Schema.decodeUnknownSync(SpeedOptions)([
				{
					availability: "always",
					consumption_basis: "usage-credit-price",
					consumption_multiplier: 6,
					default: true,
					description: "Contradictory economics.",
					id: "fast",
					input_consumption_multiplier: 2,
					label: "Fast",
					native_value: "fast",
					output_consumption_multiplier: 3,
					source_url: "https://example.test/speed",
					speed_multiplier: null,
					verified_at: "2026-07-27",
				},
			]),
		).toThrow();
	});

	it("maps native harness permissions onto a sparse uniform autonomy scale", () => {
		const permissions = Object.fromEntries(
			model_manifest.harnesses.map((harness) => [
				harness.id,
				harness.permissions.options.map(({ id, native_value }) => [id, native_value]),
			]),
		);

		expect(permissions).toEqual({
			claude: [
				["restricted", "plan"],
				["autonomous", "auto"],
				["unrestricted", "bypassPermissions"],
			],
			codex: [
				["restricted", "read-only"],
				["autonomous", "workspace-write"],
				["unrestricted", "danger-full-access"],
			],
			grok: [
				["restricted", "plan"],
				["autonomous", "auto"],
				["unrestricted", "always-approve"],
			],
			cursor: [
				["restricted", "ask"],
				["autonomous", "default"],
				["unrestricted", "force"],
			],
			hermes: [
				["autonomous", "profile"],
				["unrestricted", "yolo"],
			],
			opencode2: [
				["restricted", "artisan-restricted"],
				["autonomous", "artisan-auto"],
				["unrestricted", "artisan-unrestricted"],
			],
		});
	});

	it("does not promote Cursor permission rules into invented runtime modes", () => {
		const cursor = model_manifest.harnesses.find((harness) => harness.id === "cursor");
		const composer = model_manifest.models.find((model) => model.id === "cursor-composer-2-5");

		expect(cursor?.permissions.options).toMatchObject([
			{ availability: "always", id: "restricted", native_value: "ask" },
			{ availability: "always", id: "autonomous", native_value: "default" },
			{ availability: "dynamic", id: "unrestricted", native_value: "force" },
		]);
		expect(
			cursor?.permissions.options.some((option) => option.native_value.includes(".")),
		).toBe(false);
		expect(composer?.capabilities).toMatchObject({
			image_input: false,
			web_search: false,
		});
	});

	it("rejects unordered or unsupported permission defaults", () => {
		expect(() =>
			Schema.decodeUnknownSync(PermissionCapability)({
				default: "autonomous",
				options: [
					{
						approval_behavior: "none",
						availability: "always",
						description: "No prompts.",
						edit_scope: "host",
						id: "unrestricted",
						label: "Unrestricted",
						native_value: "bypass",
						safety_boundary: "bypassed",
					},
					{
						approval_behavior: "prompts",
						availability: "always",
						description: "Ask first.",
						edit_scope: "host",
						id: "restricted",
						label: "Read only",
						native_value: "ask",
						safety_boundary: "rules",
					},
				],
			}),
		).toThrow();
	});
});
