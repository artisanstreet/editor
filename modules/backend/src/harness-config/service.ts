import { createHash } from "node:crypto";

import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import type { ModelBehaviourActivationTiming } from "@artisan/protocol";

import {
	DeleteConfigValue,
	ReadConfigValue,
	SetConfigValue,
	type ConfigDocumentError,
} from "./document";
import {
	ConfigFileStore,
	type ConfigFileBackupError,
	type ConfigFileReadError,
	type ConfigFileReplaceError,
	type ConfigFileRestoreError,
	type ConfigFileWriteError,
} from "./file-store";
import { harness_config_key_id, HarnessConfigRegistry, type HarnessConfigKey } from "./keys";

/**
 * One service for reading, previewing, writing, and removing the harness config
 * keys Artisan owns.
 *
 * Callers name a declared key and a value. Everything underneath — locating the
 * document, parsing it without disturbing unrelated keys, fencing on the exact
 * bytes that were observed, taking a recoverable backup, publishing atomically,
 * and verifying the result — is this service's problem, not theirs.
 */

/** Reports a write aimed at a key no registry declared. */
export class HarnessConfigUndeclaredKey extends Data.TaggedError("HarnessConfigUndeclaredKey")<{
	readonly key_id: string;
}> {}

/** Reports a harness with no writable config target in this runtime. */
export class HarnessConfigUnavailable extends Data.TaggedError("HarnessConfigUnavailable")<{
	readonly harness_id: string;
}> {}

/** Reports a stored value that does not match the key's declared shape. */
export class HarnessConfigDecodeError extends Data.TaggedError("HarnessConfigDecodeError")<{
	readonly cause: unknown;
	readonly key_id: string;
}> {}

/** Every failure the service can surface to a caller. */
export type HarnessConfigError =
	| ConfigDocumentError
	| ConfigFileBackupError
	| ConfigFileReadError
	| ConfigFileReplaceError
	| ConfigFileRestoreError
	| ConfigFileWriteError
	| HarnessConfigDecodeError
	| HarnessConfigUnavailable
	| HarnessConfigUndeclaredKey;

/** Projects one key's current state and the fence a later write must present. */
export interface HarnessConfigReading<A> {
	readonly activation: ModelBehaviourActivationTiming;
	/** Absent when the document does not exist yet. */
	readonly document_hash: Option.Option<string>;
	readonly key_id: string;
	readonly target_path: string;
	/** Absent when the harness is using its own default. */
	readonly value: Option.Option<A>;
}

/** Previews one change without touching the filesystem. */
export type HarnessConfigChange<A> =
	| {
			readonly _tag: "Unchanged";
			readonly reading: HarnessConfigReading<A>;
	  }
	| {
			readonly _tag: "Change";
			readonly creates_document: boolean;
			readonly next: Option.Option<A>;
			readonly reading: HarnessConfigReading<A>;
			readonly writes_backup: boolean;
	  };

/** Reports what a write or delete actually did. */
export type HarnessConfigOutcome<A> =
	| {
			readonly _tag: "Written";
			readonly backup_path: Option.Option<string>;
			readonly reading: HarnessConfigReading<A>;
	  }
	| {
			readonly _tag: "Unchanged";
			readonly reading: HarnessConfigReading<A>;
	  }
	/** The document moved between observation and publication; nothing was written. */
	| {
			readonly _tag: "Changed";
			readonly reading: HarnessConfigReading<A>;
	  };

/** Supplies the idempotency fence for a durable write. */
export interface HarnessConfigWriteOptions {
	/**
	 * Reusing an id retries the exact same operation. A lost receipt may retry
	 * without producing a second backup or a duplicated write.
	 */
	readonly operation_id: string;
}

export class HarnessConfig extends Context.Service<
	HarnessConfig,
	{
		readonly Delete: <A>(
			key: HarnessConfigKey<A>,
			options: HarnessConfigWriteOptions,
		) => Effect.Effect<HarnessConfigOutcome<A>, HarnessConfigError>;
		readonly Diff: <A>(
			key: HarnessConfigKey<A>,
			next: Option.Option<A>,
		) => Effect.Effect<HarnessConfigChange<A>, HarnessConfigError>;
		readonly Read: <A>(
			key: HarnessConfigKey<A>,
		) => Effect.Effect<HarnessConfigReading<A>, HarnessConfigError>;
		readonly Write: <A>(
			key: HarnessConfigKey<A>,
			value: A,
			options: HarnessConfigWriteOptions,
		) => Effect.Effect<HarnessConfigOutcome<A>, HarnessConfigError>;
	}
>()("Artisan/HarnessConfig") {}

/**
 * Derives a backup name from the operation and the bytes it was authorized
 * against, so a retry of the same operation recovers its own backup and a
 * different operation can never adopt one.
 */
const backup_name = (
	key_id: string,
	operation_id: string,
	document_hash: Option.Option<string>,
) => {
	const identity = createHash("sha256")
		.update(
			JSON.stringify({
				document_hash: Option.getOrElse(document_hash, () => "absent"),
				key_id,
				operation_id,
			}),
		)
		.digest("hex");

	return `harness-config-${identity}`;
};

const same_value = (left: Option.Option<unknown>, right: Option.Option<unknown>) =>
	JSON.stringify(Option.getOrUndefined(left)) === JSON.stringify(Option.getOrUndefined(right));

export const HarnessConfigLive = Layer.effect(
	HarnessConfig,
	Effect.gen(function* () {
		const files = yield* ConfigFileStore;
		const registry = yield* HarnessConfigRegistry;

		/** Resolves the declared key against the registry before any file is opened. */
		const Target = <A>(key: HarnessConfigKey<A>) =>
			Effect.gen(function* () {
				const key_id = harness_config_key_id(key);

				if (!registry.Declares(key)) {
					return yield* new HarnessConfigUndeclaredKey({ key_id });
				}

				const target = registry.FindTarget(key.harness_id);

				if (Option.isNone(target)) {
					return yield* new HarnessConfigUnavailable({ harness_id: key.harness_id });
				}

				return { key_id, target: target.value };
			});

		const Observe = <A>(key: HarnessConfigKey<A>) =>
			Effect.gen(function* () {
				const { key_id, target } = yield* Target(key);
				const snapshot = yield* files.Read(target.path);
				const content = Option.match(snapshot, {
					onNone: () => "",
					onSome: (found) => found.content,
				});
				const raw = yield* ReadConfigValue(target.format, content, key.path);
				const value = Option.isNone(raw)
					? Option.none<A>()
					: Option.some(
							yield* Schema.decodeUnknownEffect(key.schema)(raw.value).pipe(
								Effect.mapError(
									(cause) => new HarnessConfigDecodeError({ cause, key_id }),
								),
							),
						);

				return {
					content,
					reading: {
						activation: key.activation,
						document_hash: Option.map(snapshot, (found) => found.content_hash),
						key_id,
						target_path: target.path,
						value,
					} satisfies HarnessConfigReading<A>,
					target,
				};
			});

		const Read = <A>(key: HarnessConfigKey<A>) =>
			Observe(key).pipe(Effect.map((observed) => observed.reading));

		const Diff = <A>(key: HarnessConfigKey<A>, next: Option.Option<A>) =>
			Observe(key).pipe(
				Effect.map(
					(observed): HarnessConfigChange<A> =>
						same_value(observed.reading.value, next)
							? { _tag: "Unchanged", reading: observed.reading }
							: {
									_tag: "Change",
									creates_document: Option.isNone(observed.reading.document_hash),
									next,
									reading: observed.reading,
									writes_backup: Option.isSome(observed.reading.document_hash),
								},
				),
			);

		/** Publishes one already-serialized document behind the observed-bytes fence. */
		const Publish = <A>(input: {
			readonly content: string;
			readonly key: HarnessConfigKey<A>;
			readonly next: Option.Option<A>;
			readonly operation_id: string;
			readonly reading: HarnessConfigReading<A>;
			readonly target: { readonly backups_directory: string; readonly path: string };
		}) =>
			Effect.gen(function* () {
				const publication = yield* files.ReplaceAtomic({
					backups_directory: input.target.backups_directory,
					backup_name: backup_name(
						input.reading.key_id,
						input.operation_id,
						input.reading.document_hash,
					),
					content: input.content,
					...(Option.isNone(input.reading.document_hash)
						? {}
						: { expected_content_hash: input.reading.document_hash.value }),
					path: input.target.path,
				});

				if (publication._tag === "Changed") {
					const observed = yield* Observe(input.key);

					return { _tag: "Changed" as const, reading: observed.reading };
				}

				const published = yield* Observe(input.key);

				return same_value(published.reading.value, input.next)
					? ({
							_tag: "Written" as const,
							backup_path: Option.fromUndefinedOr(publication.backup_path),
							reading: published.reading,
						} satisfies HarnessConfigOutcome<A>)
					: { _tag: "Changed" as const, reading: published.reading };
			});

		const Write = <A>(key: HarnessConfigKey<A>, value: A, options: HarnessConfigWriteOptions) =>
			Effect.gen(function* () {
				const observed = yield* Observe(key);
				const next = Option.some(value);

				if (same_value(observed.reading.value, next)) {
					return { _tag: "Unchanged" as const, reading: observed.reading };
				}

				const encoded = yield* Schema.encodeUnknownEffect(key.schema)(value).pipe(
					Effect.mapError(
						(cause) =>
							new HarnessConfigDecodeError({
								cause,
								key_id: observed.reading.key_id,
							}),
					),
				);
				const content = yield* SetConfigValue(
					observed.target.format,
					observed.content,
					key.path,
					encoded,
				);

				return yield* Publish({
					content,
					key,
					next,
					operation_id: options.operation_id,
					reading: observed.reading,
					target: observed.target,
				});
			});

		const Delete = <A>(key: HarnessConfigKey<A>, options: HarnessConfigWriteOptions) =>
			Effect.gen(function* () {
				const observed = yield* Observe(key);

				if (Option.isNone(observed.reading.value)) {
					return { _tag: "Unchanged" as const, reading: observed.reading };
				}

				const content = yield* DeleteConfigValue(
					observed.target.format,
					observed.content,
					key.path,
				);

				return yield* Publish({
					content,
					key,
					next: Option.none<A>(),
					operation_id: options.operation_id,
					reading: observed.reading,
					target: observed.target,
				});
			});

		return { Delete, Diff, Read, Write };
	}),
);
