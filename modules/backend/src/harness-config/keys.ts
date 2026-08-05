import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import type { ModelBehaviourActivationTiming } from "@artisan/protocol";

import type { ConfigDocumentFormat, ConfigKeyPath } from "./document";

/**
 * The declared set of harness config keys Artisan is allowed to touch.
 *
 * This registry is the product boundary. Artisan does not expose a generated
 * form over every key a harness supports; it owns a curated list, and a write
 * to anything outside that list fails before a file is opened. Declaring a key
 * here is the deliberate act of taking ownership of it.
 */

/**
 * Identifies one declared key independently of the type it carries.
 *
 * The registry holds identities rather than typed keys: a heterogeneous list of
 * `HarnessConfigKey<A>` would need a cast at every use, and the registry only
 * ever needs to answer "is this declared, and where does it live".
 */
export interface HarnessConfigKeyIdentity {
	/** When a changed value becomes observable to the user. */
	readonly activation: ModelBehaviourActivationTiming;
	/** Product-facing explanation of the behaviour, not the native key name. */
	readonly description: string;
	readonly harness_id: string;
	readonly path: ConfigKeyPath;
}

/** Describes one key Artisan owns inside a harness config document. */
export interface HarnessConfigKey<A> extends HarnessConfigKeyIdentity {
	readonly schema: Schema.Codec<A, unknown, never, never>;
}

/** Identifies a declared key by harness and dotted path. */
export const harness_config_key_id = (key: HarnessConfigKeyIdentity) =>
	`${key.harness_id}:${key.path.join(".")}`;

/** Locates one harness's config document and its backup directory. */
export interface HarnessConfigTarget {
	readonly backups_directory: string;
	readonly format: ConfigDocumentFormat;
	readonly harness_id: string;
	readonly path: string;
}

/** Reports a registry construction failure during layer assembly. */
export class HarnessConfigRegistryError extends Data.TaggedError("HarnessConfigRegistryError")<{
	readonly message: string;
}> {}

/** Owns the declared keys and per-harness targets for one backend runtime. */
export class HarnessConfigRegistry extends Context.Service<
	HarnessConfigRegistry,
	{
		readonly Keys: ReadonlyArray<HarnessConfigKeyIdentity>;
		readonly Declares: (key: HarnessConfigKeyIdentity) => boolean;
		readonly FindTarget: (harness_id: string) => Option.Option<HarnessConfigTarget>;
		readonly Targets: ReadonlyArray<HarnessConfigTarget>;
	}
>()("Artisan/HarnessConfigRegistry") {}

/**
 * Declares Codex's non-plan-mode user-input feature.
 *
 * Codex reads this when a thread starts, so an in-flight turn keeps the
 * behaviour it began with. Artisan's own question surface already renders the
 * request; this only decides whether the harness is allowed to raise one
 * outside plan mode.
 */
export const CodexRequestUserInput: HarnessConfigKey<boolean> = {
	activation: "new_threads",
	description:
		"Let the agent pause and ask you a question outside plan mode instead of assuming an answer.",
	harness_id: "codex",
	path: ["features", "default_mode_request_user_input"],
	schema: Schema.Boolean,
};

/**
 * Describes Codex's auto-compaction trigger.
 *
 * This is the threshold at which history is summarized. It is not the model's
 * context capacity, and raising it does not buy a larger window.
 *
 * Deliberately absent from {@link DeclaredHarnessConfigKeys}: the Model
 * Behaviour adapter still owns this key, and two writers for one key is how a
 * write war starts. It is defined here as the migration target, and stays
 * unwritable through this service — an undeclared key fails closed — until that
 * adapter is moved across.
 */
export const CodexAutoCompactionTriggerTokens: HarnessConfigKey<number> = {
	activation: "new_threads",
	description:
		"Token threshold that triggers automatic history compaction; this does not change model context capacity.",
	harness_id: "codex",
	path: ["model_auto_compact_token_limit"],
	schema: Schema.Number,
};

/**
 * The keys Artisan owns across every harness it can configure.
 *
 * Adding an entry is the act of taking ownership of that key. Anything absent
 * from this list cannot be written through the service at all.
 */
export const DeclaredHarnessConfigKeys: ReadonlyArray<HarnessConfigKeyIdentity> = [
	CodexRequestUserInput,
];

/** Provides a registry with no writable target, so every write fails closed. */
export const EmptyHarnessConfigRegistryLive = Layer.succeed(HarnessConfigRegistry, {
	Declares: () => false,
	FindTarget: () => Option.none(),
	Keys: [],
	Targets: [],
});

/** Builds a registry and rejects ambiguous key or target ownership. */
export const MakeHarnessConfigRegistryLayer = (input: {
	readonly keys?: ReadonlyArray<HarnessConfigKeyIdentity>;
	readonly targets: ReadonlyArray<HarnessConfigTarget>;
}) =>
	Layer.effect(
		HarnessConfigRegistry,
		Effect.gen(function* () {
			const keys = input.keys ?? DeclaredHarnessConfigKeys;
			const key_ids = keys.map((key) => harness_config_key_id(key));

			if (new Set(key_ids).size !== key_ids.length) {
				return yield* new HarnessConfigRegistryError({
					message: "Harness config keys must be unique",
				});
			}

			const harness_ids = input.targets.map((target) => target.harness_id);

			if (new Set(harness_ids).size !== harness_ids.length) {
				return yield* new HarnessConfigRegistryError({
					message: "Each harness may declare only one config target",
				});
			}

			const declared = new Set(key_ids);
			const by_harness = new Map(
				input.targets.map((target) => [target.harness_id, target] as const),
			);

			return {
				Declares: (key: HarnessConfigKeyIdentity) =>
					declared.has(harness_config_key_id(key)),
				FindTarget: (harness_id: string) =>
					Option.fromUndefinedOr(by_harness.get(harness_id)),
				Keys: keys,
				Targets: input.targets,
			};
		}),
	);
