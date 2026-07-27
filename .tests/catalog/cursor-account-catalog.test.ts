import {
	ModelManifest,
	make_cursor_account_catalog,
	merge_cursor_account_catalog,
	model_manifest,
} from "@artisan/catalog";
import { Schema } from "effect";
import { describe, expect, it } from "vitest";

describe("Cursor account model catalog", () => {
	it("materializes every unique native configuration without aliasing it", () => {
		const catalog = make_cursor_account_catalog([
			"gpt-5.6-sol-medium",
			"claude-opus-4-8-thinking-max",
			"gemini-3.1-pro",
			"kimi-k2.5",
			"deepseek-v3.1",
			"glm-5",
			"minimax-m2.5",
			"mistral-large",
			"llama-4-maverick",
			"future-lab-model-fast",
			"gpt-5.6-sol-medium",
		]);

		expect(catalog.models.map((model) => model.native_model_id)).toEqual([
			"claude-opus-4-8-thinking-max",
			"deepseek-v3.1",
			"future-lab-model-fast",
			"gemini-3.1-pro",
			"glm-5",
			"gpt-5.6-sol-medium",
			"kimi-k2.5",
			"llama-4-maverick",
			"minimax-m2.5",
			"mistral-large",
		]);
		expect(catalog.providers.map((provider) => provider.id)).toEqual([
			"anthropic",
			"deepseek",
			"google",
			"meta",
			"minimax",
			"mistral",
			"moonshot",
			"openai",
			"unknown",
			"zhipu",
		]);
	});

	it("prefers provider metadata supplied by account discovery", () => {
		const catalog = make_cursor_account_catalog([
			"future-lab-model",
			{
				native_model_id: "future-lab-model",
				provider: { id: "future-lab", label: "Future Lab" },
			},
		]);

		expect(catalog.models).toMatchObject([
			{ native_model_id: "future-lab-model", provider: "future-lab" },
		]);
		expect(catalog.providers).toEqual([{ id: "future-lab", label: "Future Lab" }]);
	});

	it("keeps encoded reasoning and Fast variants native without inventing controls", () => {
		const catalog = make_cursor_account_catalog([
			"gpt-5.6-sol-low-fast",
			"claude-opus-4-8-thinking-max",
		]);
		const fast = catalog.models.find(
			(model) => model.native_model_id === "gpt-5.6-sol-low-fast",
		);
		const max = catalog.models.find(
			(model) => model.native_model_id === "claude-opus-4-8-thinking-max",
		);

		expect(fast?.capabilities.thinking).toMatchObject({
			availability: "native",
		});
		expect(fast?.capabilities.speed_options).toMatchObject([
			{ id: "fast", speed_multiplier: null },
		]);
		expect(max?.capabilities.thinking).toMatchObject({
			availability: "native",
		});
	});

	it("merges the live account inventory while retaining curated metadata", () => {
		const merged = merge_cursor_account_catalog(model_manifest, [
			"composer-2.5",
			"gpt-5.6-sol-medium",
			"kimi-k2.5",
		]);

		expect(Schema.decodeUnknownSync(ModelManifest)(merged)).toEqual(merged);
		expect(
			merged.models.filter((model) => model.native_model_id === "composer-2.5"),
		).toHaveLength(1);
		expect(
			merged.models
				.filter((model) => model.harness === "cursor")
				.map((model) => model.native_model_id),
		).toEqual(["composer-2.5", "auto", "cursor-grok-4.5", "gpt-5.6-sol-medium", "kimi-k2.5"]);
		expect(merged.providers.map((provider) => provider.id)).toContain("moonshot");
		expect(
			merged.models.find((model) => model.id === "cursor-auto")?.disabled?.reason,
		).toContain("did not return");
		expect(
			merged.models
				.find((model) => model.id === "cursor-composer-2-5")
				?.capabilities.speed_options.find((option) => option.id === "fast")?.disabled
				?.reason,
		).toContain("did not return a Fast configuration");
		expect(
			merged.models
				.find((model) => model.id === "cursor-composer-2-5")
				?.capabilities.speed_options.find((option) => option.default)?.id,
		).toBe("standard");
	});

	it("refreshes idempotently and removes providers from stale discoveries", () => {
		const first = merge_cursor_account_catalog(model_manifest, ["kimi-k2.5", "gemini-3.1-pro"]);
		const second = merge_cursor_account_catalog(first, ["gemini-3.1-pro"]);
		const third = merge_cursor_account_catalog(second, ["gemini-3.1-pro"]);

		expect(
			second.models
				.filter((model) => model.id.startsWith("cursor-account-"))
				.map((model) => model.native_model_id),
		).toEqual(["gemini-3.1-pro"]);
		expect(second.providers.map((provider) => provider.id)).not.toContain("moonshot");
		expect(third.models).toEqual(second.models);
		expect(third.providers).toEqual(second.providers);
		expect(third.revision).toBe(second.revision);
	});
});
