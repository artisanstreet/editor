import { TomlDocument } from "@decimalturn/toml-patch";
import { Data, Effect, Schema } from "effect";

import { AutoCompactionTriggerTokens, type ModelBehaviourValue } from "@artisan/protocol";

import { hash_model_behaviour_value } from "./model-behaviour-value";

/** Names the only Codex config key owned by the initial Model Behaviour registry. */
export const codex_auto_compaction_native_key = "model_auto_compact_token_limit";

/** Reports malformed TOML or an invalid owned value without exposing adjacent config. */
export class CodexModelBehaviourConfigError extends Data.TaggedError(
	"CodexModelBehaviourConfigError",
)<{
	readonly cause: unknown;
	readonly operation: "parse" | "patch";
}> {}

/** Returns one owned Codex value and its content-free canonical identity. */
export interface CodexModelBehaviourValue {
	readonly hash: string;
	readonly value: ModelBehaviourValue;
}

function parse_document(content: string) {
	return new TomlDocument(content, { integersAsBigInt: false });
}

function read_value(document: TomlDocument) {
	const config = document.toJsObject as Readonly<Record<string, unknown>>;
	const native_value = config[codex_auto_compaction_native_key];
	const value: ModelBehaviourValue =
		native_value === undefined
			? { type: "provider_default" }
			: {
					type: "integer",
					value: Schema.decodeUnknownSync(AutoCompactionTriggerTokens)(native_value),
				};

	return { hash: hash_model_behaviour_value(value), value } satisfies CodexModelBehaviourValue;
}

/** Parses only Artisan's owned compaction setting from a complete Codex TOML document. */
export function read_codex_model_behaviour(content: string) {
	return Effect.try({
		catch: (cause) => new CodexModelBehaviourConfigError({ cause, operation: "parse" }),
		try: () => read_value(parse_document(content)),
	});
}

/** Patches the owned Codex key while retaining unrelated comments and formatting. */
export function patch_codex_model_behaviour(content: string, value: ModelBehaviourValue) {
	return Effect.try({
		catch: (cause) => new CodexModelBehaviourConfigError({ cause, operation: "patch" }),
		try: () => {
			const document = parse_document(content);
			const updated = { ...(document.toJsObject as Record<string, unknown>) };

			if (value.type === "provider_default") {
				delete updated[codex_auto_compaction_native_key];
			} else {
				updated[codex_auto_compaction_native_key] = Schema.decodeUnknownSync(
					AutoCompactionTriggerTokens,
				)(value.value);
			}

			document.patch(updated);

			const patched = document.toTomlString;
			const verified = read_value(parse_document(patched));

			if (verified.hash !== hash_model_behaviour_value(value)) {
				throw new Error("Codex config patch did not preserve the requested value");
			}

			return { content: patched, value: verified };
		},
	});
}
