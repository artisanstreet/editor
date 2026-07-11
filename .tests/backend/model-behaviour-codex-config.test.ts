import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	codex_auto_compaction_native_key,
	patch_codex_model_behaviour,
	read_codex_model_behaviour,
} from "../../modules/backend/src/model-behaviour/codex-config";

const config = [
	"# Keep this user-authored header",
	'model = "gpt-5.6"',
	"model_auto_compact_token_limit = 200000 # Existing note",
	"",
	"[model_providers.private]",
	'base_url = "https://example.test"',
	'api_key = "do-not-ingest"',
	"",
].join("\n");

describe("Codex Model Behaviour config", () => {
	it("reads only the owned key into a canonical value identity", async () => {
		const result = await Effect.runPromise(read_codex_model_behaviour(config));

		expect(result).toMatchObject({
			hash: expect.stringMatching(/^[a-f0-9]{64}$/),
			value: { type: "integer", value: 200_000 },
		});
		expect(JSON.stringify(result)).not.toContain("do-not-ingest");
	});

	it("patches one value while preserving comments, formatting, and credentials", async () => {
		const result = await Effect.runPromise(
			patch_codex_model_behaviour(config, { type: "integer", value: 250_000 }),
		);

		expect(result.content).toContain("# Keep this user-authored header");
		expect(result.content).toContain("# Existing note");
		expect(result.content).toContain('api_key = "do-not-ingest"');
		expect(result.content).toContain(`${codex_auto_compaction_native_key} = 250000`);
		expect(result.value.value).toEqual({ type: "integer", value: 250_000 });
	});

	it("removes the owned key for provider default without rewriting unrelated config", async () => {
		const result = await Effect.runPromise(
			patch_codex_model_behaviour(config, { type: "provider_default" }),
		);

		expect(result.content).not.toContain(codex_auto_compaction_native_key);
		expect(result.content).toContain('model = "gpt-5.6"');
		expect(result.content).toContain('api_key = "do-not-ingest"');
		expect(result.value.value).toEqual({ type: "provider_default" });
	});

	it("rejects malformed TOML, wrong native types, and out-of-range values", async () => {
		const malformed = await Effect.runPromise(
			read_codex_model_behaviour("broken = [").pipe(Effect.exit),
		);
		const wrong_type = await Effect.runPromise(
			read_codex_model_behaviour('model_auto_compact_token_limit = "a lot"\n').pipe(
				Effect.exit,
			),
		);
		const out_of_range = await Effect.runPromise(
			patch_codex_model_behaviour(config, {
				type: "integer",
				value: 20_000_000,
			}).pipe(Effect.exit),
		);

		expect(malformed._tag).toBe("Failure");
		expect(wrong_type._tag).toBe("Failure");
		expect(out_of_range._tag).toBe("Failure");
	});
});
