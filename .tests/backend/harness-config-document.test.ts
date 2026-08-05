import { Effect, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	DeleteConfigValue,
	ReadConfigValue,
	SetConfigValue,
} from "../../modules/backend/src/harness-config/document";

const run = <A, E>(effect: Effect.Effect<A, E>) => Effect.runPromise(effect as Effect.Effect<A>);

const run_exit = <A, E>(effect: Effect.Effect<A, E>) => Effect.runPromise(Effect.exit(effect));

describe("harness config documents", () => {
	describe("toml", () => {
		it("creates a nested table for a key the document has never carried", async () => {
			const patched = await run(
				SetConfigValue("toml", "", ["features", "default_mode_request_user_input"], true),
			);

			expect(patched).toContain("[features]");
			expect(patched).toContain("default_mode_request_user_input = true");
		});

		it("preserves comments, unrelated keys, and credentials", async () => {
			const original = [
				"# Sander's Codex configuration.",
				'model = "gpt-5.2-codex"',
				"",
				"[mcp_servers.private]",
				'command = "node"',
				'api_key = "sk-do-not-touch"',
				"",
			].join("\n");
			const patched = await run(
				SetConfigValue(
					"toml",
					original,
					["features", "default_mode_request_user_input"],
					true,
				),
			);

			expect(patched).toContain("# Sander's Codex configuration.");
			expect(patched).toContain('model = "gpt-5.2-codex"');
			expect(patched).toContain('api_key = "sk-do-not-touch"');
			expect(patched).toContain("default_mode_request_user_input = true");
		});

		it("updates an existing value in place", async () => {
			const original = "[features]\ndefault_mode_request_user_input = false\n";
			const patched = await run(
				SetConfigValue(
					"toml",
					original,
					["features", "default_mode_request_user_input"],
					true,
				),
			);

			expect(patched).toContain("default_mode_request_user_input = true");
			expect(patched).not.toContain("default_mode_request_user_input = false");
		});

		it("reads a nested value and reports absence as None", async () => {
			const content = "[features]\ndefault_mode_request_user_input = true\n";

			expect(
				await run(
					ReadConfigValue("toml", content, [
						"features",
						"default_mode_request_user_input",
					]),
				),
			).toStrictEqual(Option.some(true));
			expect(
				await run(ReadConfigValue("toml", content, ["features", "absent"])),
			).toStrictEqual(Option.none());
			expect(await run(ReadConfigValue("toml", "", ["features", "any"]))).toStrictEqual(
				Option.none(),
			);
		});

		it("deletes only the owned key and leaves the surrounding table", async () => {
			const original = [
				"[features]",
				"default_mode_request_user_input = true",
				"other_feature = true",
				"",
			].join("\n");
			const patched = await run(
				DeleteConfigValue("toml", original, [
					"features",
					"default_mode_request_user_input",
				]),
			);

			expect(patched).not.toContain("default_mode_request_user_input");
			expect(patched).toContain("other_feature = true");
		});

		/**
		 * Before `toml-patch` 3.0.1, a removal was read as a rename when the
		 * removed key's value equalled an untouched sibling's, emitting that
		 * sibling twice and producing invalid TOML
		 * (DecimalTurn/toml-patch#262). Every key below is `true`, which is
		 * exactly the shape that triggered it.
		 *
		 * Kept as the guard against a downgrade: this fails here rather than a
		 * user's config being silently corrupted.
		 */
		it("removes a key followed by siblings without duplicating them", async () => {
			const original = [
				"[features]",
				"default_mode_request_user_input = true",
				"second = true",
				"third = true",
				"",
			].join("\n");
			const patched = await run(
				DeleteConfigValue("toml", original, [
					"features",
					"default_mode_request_user_input",
				]),
			);

			expect(patched.match(/second = true/g)).toHaveLength(1);
			expect(patched.match(/third = true/g)).toHaveLength(1);
			expect(
				await run(ReadConfigValue("toml", patched, ["features", "second"])),
			).toStrictEqual(Option.some(true));
		});

		it("keeps comments that belong to surrounding keys when removing one", async () => {
			const original = [
				"# top of file",
				"[features]",
				"# explains the owned key",
				"default_mode_request_user_input = true",
				"# explains the neighbour",
				"second = true",
				"",
			].join("\n");
			const patched = await run(
				DeleteConfigValue("toml", original, [
					"features",
					"default_mode_request_user_input",
				]),
			);

			expect(patched).toContain("# top of file");
			expect(patched).toContain("# explains the neighbour");
			expect(patched).not.toContain("default_mode_request_user_input");
			/**
			 * 3.0.0's comment ownership takes the removed key's own comment with
			 * it, rather than stranding a note describing a key that is gone.
			 */
			expect(patched).not.toContain("# explains the owned key");
		});

		it("removes a dotted root key without a table header", async () => {
			const original = 'features.default_mode_request_user_input = true\nmodel = "gpt"\n';
			const patched = await run(
				DeleteConfigValue("toml", original, [
					"features",
					"default_mode_request_user_input",
				]),
			);

			expect(patched).not.toContain("default_mode_request_user_input");
			expect(patched).toContain('model = "gpt"');
		});

		it("removes a key held inside an inline table", async () => {
			const patched = await run(
				DeleteConfigValue("toml", "features = { enabled = true, other = true }\n", [
					"features",
					"enabled",
				]),
			);

			expect(patched).not.toContain("enabled");
			expect(patched).toContain("other = true");
		});

		/**
		 * Removing the intermediate container along with the leaf makes
		 * `patch()` throw upstream. Only the leaf is ever removed here, which
		 * keeps this shape working and lets the library elide the emptied table
		 * on its own.
		 */
		it("removes a dotted key nested inside a table header", async () => {
			const patched = await run(
				DeleteConfigValue("toml", "[a]\nb.c = true\nd = 1\n", ["a", "b", "c"]),
			);

			expect(await run(ReadConfigValue("toml", patched, ["a", "b", "c"]))).toStrictEqual(
				Option.none(),
			);
			expect(await run(ReadConfigValue("toml", patched, ["a", "d"]))).toStrictEqual(
				Option.some(1),
			);
		});

		it("deleting an absent key is a no-op rather than a failure", async () => {
			const patched = await run(
				DeleteConfigValue("toml", 'model = "gpt-5.2-codex"\n', ["features", "absent"]),
			);

			expect(patched).toContain('model = "gpt-5.2-codex"');
		});

		it("rejects a malformed document instead of rewriting it", async () => {
			const exit = await run_exit(
				SetConfigValue("toml", "[features\nbroken", ["features", "enabled"], true),
			);

			expect(exit._tag).toBe("Failure");
		});

		it("writes a top-level integer key", async () => {
			const patched = await run(
				SetConfigValue("toml", "", ["model_auto_compact_token_limit"], 120_000),
			);

			expect(patched).toContain("model_auto_compact_token_limit = 120000");
		});
	});

	describe("json", () => {
		it("creates a nested object in an absent document", async () => {
			const patched = await run(SetConfigValue("json", "", ["features", "enabled"], true));

			expect(JSON.parse(patched)).toStrictEqual({ features: { enabled: true } });
			expect(patched).toContain('\n  "features"');
		});

		it("preserves unrelated keys and the document's own indentation", async () => {
			const original = '{\n  "model": "opus",\n  "apiKeyHelper": "secret.sh"\n}\n';
			const patched = await run(
				SetConfigValue("json", original, ["features", "enabled"], true),
			);

			expect(JSON.parse(patched)).toStrictEqual({
				apiKeyHelper: "secret.sh",
				features: { enabled: true },
				model: "opus",
			});
			expect(patched).toContain('\n  "model"');
			expect(patched.endsWith("\n")).toBe(true);
		});

		it("removes an owned key without disturbing its siblings", async () => {
			const original = '{\n  "features": {\n    "enabled": true,\n    "other": 1\n  }\n}\n';
			const patched = await run(DeleteConfigValue("json", original, ["features", "enabled"]));

			expect(JSON.parse(patched)).toStrictEqual({ features: { other: 1 } });
		});

		/**
		 * `JSON.parse` rejects comments, so a JSONC document fails closed rather
		 * than being silently republished without the user's annotations.
		 */
		it("refuses to rewrite a document carrying comments", async () => {
			const exit = await run_exit(
				SetConfigValue(
					"json",
					'{\n  // keep me\n  "model": "opus"\n}\n',
					["features", "enabled"],
					true,
				),
			);

			expect(exit._tag).toBe("Failure");
		});
	});
});
