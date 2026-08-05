import { Effect, Option, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	CodexAutoCompactionTriggerTokens,
	CodexRequestUserInput,
	DeclaredHarnessConfigKeys,
	harness_config_key_id,
	HarnessConfigRegistry,
	MakeHarnessConfigRegistryLayer,
	type HarnessConfigKeyIdentity,
	type HarnessConfigTarget,
} from "../../modules/backend/src/harness-config/keys";

const codex_target: HarnessConfigTarget = {
	backups_directory: "/tmp/backups",
	format: "toml",
	harness_id: "codex",
	path: "/tmp/config.toml",
};

const make_registry = (input: {
	readonly keys?: ReadonlyArray<HarnessConfigKeyIdentity>;
	readonly targets: ReadonlyArray<HarnessConfigTarget>;
}) =>
	Effect.runPromise(
		Effect.exit(
			Effect.service(HarnessConfigRegistry).pipe(
				Effect.provide(MakeHarnessConfigRegistryLayer(input)),
			),
		),
	);

describe("harness config registry", () => {
	it("identifies a key by harness and dotted path", () => {
		expect(harness_config_key_id(CodexRequestUserInput)).toBe(
			"codex:features.default_mode_request_user_input",
		);
		expect(harness_config_key_id(CodexAutoCompactionTriggerTokens)).toBe(
			"codex:model_auto_compact_token_limit",
		);
	});

	it("declares only the curated keys and resolves their target", async () => {
		const exit = await make_registry({ targets: [codex_target] });

		expect(exit._tag).toBe("Success");

		if (exit._tag !== "Success") return;

		expect(exit.value.Declares(CodexRequestUserInput)).toBe(true);
		expect(exit.value.Keys).toHaveLength(DeclaredHarnessConfigKeys.length);
		expect(exit.value.FindTarget("codex")).toStrictEqual(Option.some(codex_target));
		expect(exit.value.FindTarget("claude")).toStrictEqual(Option.none());
	});

	/**
	 * The Model Behaviour adapter still owns the auto-compaction key. Declaring
	 * it here as well would give one key two writers, so it stays defined but
	 * undeclared until that adapter migrates.
	 */
	it("leaves the auto-compaction key unwritable while another adapter owns it", async () => {
		const exit = await make_registry({ targets: [codex_target] });

		expect(exit._tag).toBe("Success");

		if (exit._tag !== "Success") return;

		expect(exit.value.Declares(CodexAutoCompactionTriggerTokens)).toBe(false);
	});

	it("does not declare a key outside the curated list", async () => {
		const exit = await make_registry({ targets: [codex_target] });

		expect(exit._tag).toBe("Success");

		if (exit._tag !== "Success") return;

		expect(
			exit.value.Declares({
				activation: "immediate",
				description: "Not owned by Artisan.",
				harness_id: "codex",
				path: ["sandbox_workspace_write", "network_access"],
			}),
		).toBe(false);
	});

	it("rejects two declarations of the same key", async () => {
		const duplicate = {
			activation: "immediate" as const,
			description: "A second claim on the same key.",
			harness_id: CodexRequestUserInput.harness_id,
			path: CodexRequestUserInput.path,
			schema: Schema.Boolean,
		};
		const exit = await make_registry({
			keys: [CodexRequestUserInput, duplicate],
			targets: [codex_target],
		});

		expect(exit._tag).toBe("Failure");
	});

	it("rejects two config targets for one harness", async () => {
		const exit = await make_registry({
			targets: [codex_target, { ...codex_target, path: "/tmp/other.toml" }],
		});

		expect(exit._tag).toBe("Failure");
	});
});
