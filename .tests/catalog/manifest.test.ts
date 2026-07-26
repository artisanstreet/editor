import {
	ModelManifest,
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

	it("maps provider-native low values into unified Light", () => {
		const supported = model_manifest.models
			.map((model) => model.capabilities.thinking)
			.filter((thinking) => thinking.availability === "supported");

		for (const thinking of supported) {
			const light = thinking.options.find((option) => option.id === "light");
			expect(light?.native_value).toBe("low");
		}
	});

	it("keeps max exceptional and model-specific", () => {
		const models_with_max = model_manifest.models
			.filter(
				(model) =>
					model.capabilities.thinking.availability === "supported" &&
					model.capabilities.thinking.options.some((option) => option.id === "max"),
			)
			.map((model) => model.id);

		expect(models_with_max).toEqual([
			"codex-sol",
			"codex-terra",
			"codex-luna",
			"claude-fable",
			"claude-opus",
			"claude-sonnet",
		]);
	});

	it("lists every first-party model exposed by the supported coding harnesses", () => {
		const native_models_by_harness = (harness: "codex" | "claude" | "grok") =>
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
		expect(native_models_by_harness("grok")).toEqual(["grok-4.5"]);
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

	it("models every OpenCode gateway without flattening models", () => {
		const opencode = model_manifest.harnesses.find((harness) => harness.id === "opencode");
		const modeled_gateways = model_manifest.models.flatMap((model) =>
			model.harness === "opencode" && model.routing.kind === "gateway"
				? [model.routing.gateway_id]
				: [],
		);

		expect(modeled_gateways).toEqual(opencode?.gateways.map((gateway) => gateway.id));
		expect(model_manifest.models.find((model) => model.id === "opencode-qwen")).toMatchObject({
			name: "Qwen Coder",
			routing: { gateway_id: "alibaba", kind: "gateway" },
		});
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
					default: true,
					description: "Ordinary route.",
					id: "standard",
					label: "Standard",
					native_value: "standard",
				},
				{
					default: true,
					description: "Another route.",
					id: "other",
					label: "Other",
					native_value: "standard",
				},
			]),
		).toThrow();
	});

	it("models speed as ordered provider-native options", () => {
		const sol = model_manifest.models.find((model) => model.id === "codex-sol");

		expect(sol?.capabilities.speed_options).toEqual([
			{
				default: true,
				description: "Standard service tier.",
				id: "standard",
				label: "Standard",
				native_value: "standard",
			},
			{
				default: false,
				description: "Prioritizes lower latency through the provider's fast service tier.",
				id: "fast",
				label: "Fast",
				native_value: "fast",
			},
		]);
		expect(
			model_manifest.models
				.filter((model) => model.id !== "codex-sol")
				.every((model) => model.capabilities.speed_options.length === 1),
		).toBe(true);
	});

	it("accepts arbitrary future speed tiers without changing the schema", () => {
		const options = Schema.decodeUnknownSync(SpeedOptions)([
			{
				default: false,
				description: "Trades latency for lower cost.",
				id: "economy",
				label: "Economy",
				native_value: "provider-economy",
			},
			{
				default: true,
				description: "The provider default.",
				id: "standard",
				label: "Standard",
				native_value: "provider-standard",
			},
			{
				default: false,
				description: "Runs on a dedicated inference backend.",
				id: "accelerated",
				label: "Accelerated",
				native_value: "provider-accelerated",
			},
		]);

		expect(options.map((option) => option.id)).toEqual(["economy", "standard", "accelerated"]);
	});
});
