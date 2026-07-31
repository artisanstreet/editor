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

	it("uses xhigh as the canonical ID and Extra High as its label", () => {
		expect(thinking_level_labels.xhigh).toBe("Extra High");
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

	it("keeps max exceptional and limited to documented Claude models", () => {
		const models_with_max = model_manifest.models
			.filter(
				(model) =>
					model.capabilities.thinking.availability === "supported" &&
					model.capabilities.thinking.options.some((option) => option.id === "max"),
			)
			.map((model) => model.id);

		expect(models_with_max).toEqual([
			"claude-fable",
			"claude-opus",
			"claude-sonnet",
			"cursor-claude-fable-5",
			"cursor-claude-opus-5",
			"cursor-claude-sonnet-5",
			"cursor-kimi-k3",
			"cursor-glm-5-2",
		]);
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
			"grok-4.5",
			"grok-4.3",
			"grok-build-0.1",
			"composer-2.5",
		]);
		expect(native_models_by_harness("cursor")).toEqual([
			"composer-2.5",
			"auto",
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
			"gemini-3.6-flash",
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
					{ economics: "standard", id: "high", native_value: "high" },
					{ economics: "standard", id: "light", native_value: "low" },
				],
			}),
		).toThrow();
	});

	it("allows sparse native levels without inventing intermediate support", () => {
		const sparse = Schema.decodeUnknownSync(SupportedThinkingCapability)({
			availability: "supported",
			default: "high",
			options: [
				{ economics: "standard", id: "light", native_value: "low" },
				{ economics: "standard", id: "high", native_value: "high" },
			],
		});

		expect(sparse.options.map((option) => option.id)).toEqual(["light", "high"]);
	});

	it("contains the four primary coding harnesses and providers", () => {
		expect(model_manifest.harnesses.map((harness) => harness.id)).toEqual([
			"codex",
			"claude",
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
					{ economics: "standard", id: "light", native_value: "low" },
					{ economics: "standard", id: "high", native_value: "low" },
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
		const composer = model_manifest.models.find((model) => model.id === "cursor-composer-2-5");
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
				["supervised", "default"],
				["trusted", "acceptEdits"],
				["autonomous", "auto"],
				["unrestricted", "bypassPermissions"],
			],
			codex: [
				["restricted", "read-only"],
				["supervised", "workspace-write"],
				["autonomous", "workspace-write-no-prompts"],
				["unrestricted", "danger-full-access"],
			],
			grok: [
				["supervised", "ask"],
				["autonomous", "auto"],
				["unrestricted", "always-approve"],
			],
			cursor: [
				["supervised", "default"],
				["unrestricted", "force"],
			],
		});
	});

	it("does not promote Cursor permission rules into invented runtime modes", () => {
		const cursor = model_manifest.harnesses.find((harness) => harness.id === "cursor");
		const composer = model_manifest.models.find((model) => model.id === "cursor-composer-2-5");

		expect(cursor?.permissions.options).toMatchObject([
			{ availability: "always", id: "supervised", native_value: "default" },
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
				default: "trusted",
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
						id: "supervised",
						label: "Supervised",
						native_value: "ask",
						safety_boundary: "rules",
					},
				],
			}),
		).toThrow();
	});
});
