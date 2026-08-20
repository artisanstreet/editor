import { createHash, randomUUID } from "node:crypto";

import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { FileDescriptorOf } from "../filesystem/node/file-descriptor";
import {
	Cause,
	Context,
	Data,
	Effect,
	Exit,
	FileSystem,
	Layer,
	Option,
	Path,
	PlatformError,
	Result,
	Schema,
} from "effect";

import {
	make_private_file_permissions_layer,
	PosixPrivateFilePermissionsSnapshot,
	PrivateFilePermissions,
	PrivateFilePermissionsPlatform,
	ReadPrivateFileIdentity,
	type PrivateFileIdentity,
	type PrivateFilePermissionsSnapshot,
	WindowsPrivateFilePermissionsSnapshot,
} from "./private-file-permissions";

/** Reports a failed config-file read. */
export class ConfigFileReadError extends Data.TaggedError("ConfigFileReadError")<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Reports a failed deterministic backup operation. */
export class ConfigFileBackupError extends Data.TaggedError("ConfigFileBackupError")<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Reports a failed conditional publication. */
export class ConfigFileReplaceError extends Data.TaggedError("ConfigFileReplaceError")<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Reports a failed restoration after publication failure. */
export class ConfigFileRestoreError extends Data.TaggedError("ConfigFileRestoreError")<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Reports a failed target-file write. */
export class ConfigFileWriteError extends Data.TaggedError("ConfigFileWriteError")<{
	readonly cause: unknown;
	readonly path: string;
}> {}

/** Describes the exact bytes currently stored in one config file. */
export interface ConfigFileSnapshot {
	readonly content: string;
	readonly content_hash: string;
	readonly modified_at: string;
	readonly path: string;
}

/** Supplies the inputs for a create-if-absent or conditional replacement. */
export interface ConfigFileReplaceOptions {
	readonly backups_directory: string;
	readonly backup_name: string;
	readonly content: string;
	readonly expected_content_hash?: string;
	readonly path: string;
}

/** Describes a successful or rejected conditional config-file publication. */
export type ConfigFileReplaceResult =
	| ({ readonly _tag: "Written" } & ConfigFileSnapshot & {
				readonly backup_path?: string;
			})
	| {
			readonly _tag: "Changed";
			readonly path: string;
			readonly content?: string;
			readonly content_hash?: string;
			readonly modified_at?: string;
			readonly backup_path?: string;
	  };

/** Provides deterministic publication-failure points for focused tests. */
export interface ConfigFileHooks {
	readonly after_claim?: (path: string) => Promise<void>;
	readonly after_private_truncate?: (path: string) => Promise<void>;
	readonly before_backup?: (path: string) => Promise<void>;
	readonly before_compare?: (path: string) => Promise<void>;
	readonly before_permissions?: (path: string) => Promise<void>;
	readonly before_replace?: (path: string) => Promise<void>;
	readonly before_rollback?: (path: string) => Promise<void>;
	readonly after_replace?: (path: string) => Promise<void>;
}

/** Provides ephemeral, byte-exact config-file reconciliation operations. */
export class ConfigFileStore extends Context.Service<
	ConfigFileStore,
	{
		readonly Read: (
			path: string,
		) => Effect.Effect<Option.Option<ConfigFileSnapshot>, ConfigFileReadError>;
		readonly ReplaceAtomic: (
			options: ConfigFileReplaceOptions,
		) => Effect.Effect<
			ConfigFileReplaceResult,
			| ConfigFileBackupError
			| ConfigFileReplaceError
			| ConfigFileRestoreError
			| ConfigFileWriteError
		>;
	}
>()("Artisan/HarnessConfigFileStore") {}

type FileIdentity = PrivateFileIdentity;

type PrivateWriteResult =
	| { readonly _tag: "Occupied" }
	| {
			readonly _tag: "WriteFailed";
			readonly cause: ConfigFileWriteError;
			readonly identity: FileIdentity;
	  }
	| { readonly _tag: "Written"; readonly identity: FileIdentity };

interface InternalSnapshot extends ConfigFileSnapshot {
	readonly bytes: Uint8Array;
	readonly identity: FileIdentity;
}

const text_decoder = new TextDecoder();
const text_encoder = new TextEncoder();
const StoredPermissionsSnapshot = Schema.Union([
	Schema.Struct({
		_tag: Schema.Literal("PosixPrivateFilePermissionsSnapshot"),
		mode: Schema.Number,
	}),
	Schema.Struct({
		_tag: Schema.Literal("WindowsPrivateFilePermissionsSnapshot"),
		sddl: Schema.String,
	}),
]);

function hash_bytes(bytes: Uint8Array) {
	return createHash("sha256").update(bytes).digest("hex");
}

function is_platform_reason(cause: unknown, reason: PlatformError.SystemErrorTag) {
	return cause instanceof PlatformError.PlatformError && cause.reason._tag === reason;
}

function same_identity(left: FileIdentity, right: FileIdentity) {
	return left.inode !== 0n && left.device === right.device && left.inode === right.inode;
}

function changed(snapshot: InternalSnapshot | undefined, path: string, backup_path?: string) {
	return {
		_tag: "Changed" as const,
		path,
		...(snapshot === undefined
			? {}
			: {
					content: snapshot.content,
					content_hash: snapshot.content_hash,
					modified_at: snapshot.modified_at,
				}),
		...(backup_path === undefined ? {} : { backup_path }),
	};
}

function RunHook(hook: ((path: string) => Promise<void>) | undefined, path: string) {
	return hook === undefined
		? Effect.void
		: Effect.tryPromise({
				try: () => hook(path),
				catch: (cause) => cause,
			});
}

function ReadSnapshot(file_system: FileSystem.FileSystem, path: string) {
	return Effect.scoped(
		Effect.gen(function* () {
			const file = yield* file_system.open(path, { flag: "r" });
			const stat = yield* file.stat;
			const identity = yield* ReadPrivateFileIdentity(
				yield* FileDescriptorOf(file, "config file identity"),
			);
			const bytes =
				stat.size === 0n
					? new Uint8Array()
					: Option.getOrElse(yield* file.readAlloc(stat.size), () => new Uint8Array());

			return {
				bytes,
				content: text_decoder.decode(bytes),
				content_hash: hash_bytes(bytes),
				identity,
				modified_at: Option.getOrElse(stat.mtime, () => new Date(0)).toISOString(),
				path,
			} satisfies InternalSnapshot;
		}),
	).pipe(
		Effect.catch((cause) =>
			is_platform_reason(cause, "NotFound")
				? Effect.succeed(undefined)
				: Effect.fail(new ConfigFileReadError({ cause, path })),
		),
	);
}

function WriteOwnedBytes(
	file_system: FileSystem.FileSystem,
	path: string,
	identity: FileIdentity,
	bytes: Uint8Array,
	after_truncate?: (path: string) => Promise<void>,
) {
	return Effect.scoped(
		Effect.gen(function* () {
			const file = yield* file_system.open(path, { flag: "r+" });
			const current_identity = yield* ReadPrivateFileIdentity(
				yield* FileDescriptorOf(file, "config file identity"),
			);

			if (!same_identity(current_identity, identity)) {
				return false;
			}

			yield* file.truncate(0);
			yield* RunHook(after_truncate, path);
			yield* file.writeAll(bytes);
			yield* file.sync;

			return true;
		}),
	);
}

function WritePrivateExclusive(
	file_system: FileSystem.FileSystem,
	permissions: PrivateFilePermissions["Service"],
	path: string,
	bytes: Uint8Array,
	after_truncate?: (path: string) => Promise<void>,
) {
	return Effect.gen(function* () {
		const created = yield* permissions.CreatePrivate(path);

		if (Option.isNone(created)) {
			return { _tag: "Occupied" } satisfies PrivateWriteResult;
		}

		const written = yield* WriteOwnedBytes(
			file_system,
			path,
			created.value,
			bytes,
			after_truncate,
		).pipe(Effect.result);

		if (Result.isFailure(written)) {
			return {
				_tag: "WriteFailed",
				cause: new ConfigFileWriteError({ cause: written.failure, path }),
				identity: created.value,
			} satisfies PrivateWriteResult;
		}

		return written.success
			? ({ _tag: "Written", identity: created.value } satisfies PrivateWriteResult)
			: ({ _tag: "Occupied" } satisfies PrivateWriteResult);
	}).pipe(Effect.mapError((cause) => new ConfigFileWriteError({ cause, path })));
}

function WritePermissionsSnapshot(
	file_system: FileSystem.FileSystem,
	permissions: PrivateFilePermissions["Service"],
	path: string,
	snapshot: PrivateFilePermissionsSnapshot,
) {
	const bytes = text_encoder.encode(JSON.stringify(snapshot));

	return Effect.gen(function* () {
		const identity = yield* WritePrivateExclusive(file_system, permissions, path, bytes).pipe(
			Effect.mapError((cause) => new ConfigFileBackupError({ cause, path })),
		);

		if (identity._tag !== "Written") {
			return yield* new ConfigFileBackupError({
				cause:
					identity._tag === "WriteFailed"
						? identity.cause
						: new Error("The private permission snapshot path was already occupied"),
				path,
			});
		}
	});
}

function ReadPermissionsSnapshot(file_system: FileSystem.FileSystem, path: string) {
	return file_system.readFileString(path).pipe(
		Effect.flatMap((content) =>
			Schema.decodeUnknownEffect(Schema.fromJsonString(StoredPermissionsSnapshot), {
				onExcessProperty: "error",
			})(content),
		),
		Effect.map((snapshot) =>
			snapshot._tag === "PosixPrivateFilePermissionsSnapshot"
				? new PosixPrivateFilePermissionsSnapshot({ mode: snapshot.mode })
				: new WindowsPrivateFilePermissionsSnapshot({ sddl: snapshot.sddl }),
		),
		Effect.mapError((cause) => new ConfigFileBackupError({ cause, path })),
	);
}

function RestoreOriginal(
	file_system: FileSystem.FileSystem,
	permissions: PrivateFilePermissions["Service"],
	target: string,
	backup: InternalSnapshot,
	permissions_snapshot: PrivateFilePermissionsSnapshot,
) {
	return Effect.gen(function* () {
		const linked = yield* RestoreLink(file_system, backup.path, target);

		if (!linked) {
			return false;
		}

		const restored = yield* permissions
			.RestoreOwned(target, backup.identity, permissions_snapshot)
			.pipe(Effect.mapError((cause) => new ConfigFileRestoreError({ cause, path: target })));

		if (!restored) {
			return yield* new ConfigFileRestoreError({
				cause: new Error("The restored target changed before permissions were applied"),
				path: target,
			});
		}

		return true;
	});
}

function EraseOwned(file_system: FileSystem.FileSystem, path: string, identity: FileIdentity) {
	return Effect.scoped(
		Effect.gen(function* () {
			const file = yield* file_system.open(path, { flag: "r+" });
			const current_identity = yield* ReadPrivateFileIdentity(
				yield* FileDescriptorOf(file, "config file identity"),
			);

			if (!same_identity(current_identity, identity)) {
				return false;
			}

			yield* file.truncate(0);
			yield* file.sync;

			return true;
		}),
	).pipe(Effect.mapError((cause) => new ConfigFileRestoreError({ cause, path })));
}

function OverwriteOwned(
	file_system: FileSystem.FileSystem,
	path: string,
	identity: FileIdentity,
	bytes: Uint8Array,
) {
	return Effect.scoped(
		Effect.gen(function* () {
			const file = yield* file_system.open(path, { flag: "r+" });
			const current_identity = yield* ReadPrivateFileIdentity(
				yield* FileDescriptorOf(file, "config file identity"),
			);

			if (!same_identity(current_identity, identity)) {
				return false;
			}

			yield* file.truncate(0);
			yield* file.writeAll(bytes);
			yield* file.sync;

			return true;
		}),
	).pipe(Effect.mapError((cause) => new ConfigFileRestoreError({ cause, path })));
}

function RestrictBackupOrRestore(input: {
	readonly backup: InternalSnapshot;
	readonly backup_path: string;
	readonly file_system: FileSystem.FileSystem;
	readonly hooks: ConfigFileHooks;
	readonly permissions: PrivateFilePermissions["Service"];
	readonly permissions_snapshot: PrivateFilePermissionsSnapshot;
	readonly target: string;
}) {
	const RestrictOwned = Effect.gen(function* () {
		yield* RunHook(input.hooks.before_permissions, input.backup_path);

		const restricted = yield* input.permissions.RestrictOwned(
			input.backup_path,
			input.backup.identity,
		);

		if (!restricted) {
			return yield* new ConfigFileBackupError({
				cause: new Error("The backup changed before permissions were applied"),
				path: input.backup_path,
			});
		}
	});

	return RestrictOwned.pipe(
		Effect.catch((cause) =>
			RestoreOriginal(
				input.file_system,
				input.permissions,
				input.target,
				input.backup,
				input.permissions_snapshot,
			).pipe(
				Effect.flatMap(() =>
					Effect.fail(
						new ConfigFileBackupError({
							cause,
							path: input.backup_path,
						}),
					),
				),
			),
		),
	);
}

function ApplyTargetPermissions(input: {
	readonly file_system: FileSystem.FileSystem;
	readonly hooks: ConfigFileHooks;
	readonly identity: FileIdentity;
	readonly permissions: PrivateFilePermissions["Service"];
	readonly permissions_snapshot?: PrivateFilePermissionsSnapshot;
	readonly target: string;
}) {
	return Effect.gen(function* () {
		yield* RunHook(input.hooks.before_permissions, input.target);

		return yield* input.permissions_snapshot === undefined
			? input.permissions.RestrictOwned(input.target, input.identity)
			: input.permissions.RestoreOwned(
					input.target,
					input.identity,
					input.permissions_snapshot,
				);
	});
}

function RestoreLink(file_system: FileSystem.FileSystem, source: string, target: string) {
	return file_system.link(source, target).pipe(
		Effect.as(true),
		Effect.catch((cause) =>
			is_platform_reason(cause, "AlreadyExists")
				? Effect.succeed(false)
				: Effect.fail(new ConfigFileRestoreError({ cause, path: target })),
		),
	);
}

function MoveToBackup(
	file_system: FileSystem.FileSystem,
	target: string,
	backup_path: string,
	expected: InternalSnapshot,
) {
	return Effect.gen(function* () {
		const moved = yield* file_system.rename(target, backup_path).pipe(
			Effect.as(true),
			Effect.catch((cause) =>
				is_platform_reason(cause, "NotFound")
					? Effect.succeed(false)
					: Effect.fail(new ConfigFileBackupError({ cause, path: backup_path })),
			),
		);

		if (!moved) {
			return { _tag: "Changed" as const };
		}

		const snapshot = yield* ReadSnapshot(file_system, backup_path).pipe(
			Effect.mapError((cause) => new ConfigFileBackupError({ cause, path: backup_path })),
		);

		if (snapshot === undefined) {
			return yield* new ConfigFileBackupError({
				cause: new Error("The moved backup disappeared"),
				path: backup_path,
			});
		}

		if (
			snapshot.content_hash !== expected.content_hash ||
			!same_identity(snapshot.identity, expected.identity)
		) {
			yield* RestoreLink(file_system, backup_path, target);

			return { _tag: "Changed" as const };
		}

		return { _tag: "Moved" as const, snapshot };
	});
}

function FindRecoveryBackup(
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	backups_directory: string,
	backup_name: string,
	expected_content_hash: string,
) {
	return Effect.gen(function* () {
		const prefix = `${backup_name}.original-`;
		const entries = yield* file_system.readDirectory(backups_directory).pipe(
			Effect.catch((cause) =>
				is_platform_reason(cause, "NotFound")
					? Effect.succeed([])
					: Effect.fail(
							new ConfigFileBackupError({
								cause,
								path: backups_directory,
							}),
						),
			),
		);
		const candidates = yield* Effect.forEach(
			entries.filter(
				(entry) => entry.startsWith(prefix) && !entry.endsWith(".permissions.json"),
			),
			(entry) => {
				const path = path_service.join(backups_directory, entry);

				return ReadSnapshot(file_system, path).pipe(
					Effect.map((snapshot) => ({ path, snapshot })),
					Effect.mapError((cause) => new ConfigFileBackupError({ cause, path })),
				);
			},
		);
		const matching = candidates
			.filter((candidate) => candidate.snapshot?.content_hash === expected_content_hash)
			.sort((left, right) =>
				(right.snapshot?.modified_at ?? "").localeCompare(left.snapshot?.modified_at ?? ""),
			)[0];

		if (matching?.snapshot !== undefined) {
			const permissions_snapshot = yield* ReadPermissionsSnapshot(
				file_system,
				`${matching.path}.permissions.json`,
			);

			return {
				path: matching.path,
				permissions_snapshot,
				snapshot: matching.snapshot,
			};
		}

		if (candidates.some((candidate) => candidate.snapshot !== undefined)) {
			return yield* new ConfigFileBackupError({
				cause: new Error("The operation backup contains different bytes"),
				path: backups_directory,
			});
		}

		return undefined;
	});
}

function RollbackPublication(input: {
	readonly backup?: {
		readonly path: string;
		readonly permissions_snapshot: PrivateFilePermissionsSnapshot;
		readonly snapshot: InternalSnapshot;
	};
	readonly file_system: FileSystem.FileSystem;
	readonly hooks: ConfigFileHooks;
	readonly permissions: PrivateFilePermissions["Service"];
	readonly replacement_identity: FileIdentity;
	readonly target: string;
}) {
	return Effect.gen(function* () {
		yield* RunHook(input.hooks.before_rollback, input.target);

		if (input.backup === undefined) {
			yield* EraseOwned(input.file_system, input.target, input.replacement_identity);

			return;
		}

		const restricted = yield* input.permissions
			.RestrictOwned(input.target, input.replacement_identity)
			.pipe(
				Effect.mapError(
					(cause) => new ConfigFileRestoreError({ cause, path: input.target }),
				),
			);

		if (!restricted) {
			return;
		}

		const restored = yield* OverwriteOwned(
			input.file_system,
			input.target,
			input.replacement_identity,
			input.backup.snapshot.bytes,
		);

		if (!restored) {
			return;
		}

		yield* input.permissions
			.RestoreOwned(
				input.target,
				input.replacement_identity,
				input.backup.permissions_snapshot,
			)
			.pipe(
				Effect.mapError(
					(cause) => new ConfigFileRestoreError({ cause, path: input.target }),
				),
			);
	});
}

/** Builds the platform-independent config-file service over Effect capabilities. */
export function make_config_file_store_platform_layer(hooks: ConfigFileHooks = {}) {
	return Layer.effect(
		ConfigFileStore,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const permissions = yield* PrivateFilePermissions;

			return {
				Read: (path) =>
					ReadSnapshot(file_system, path).pipe(
						Effect.map((snapshot) => Option.fromUndefinedOr(snapshot)),
					),
				ReplaceAtomic: (options) =>
					Effect.gen(function* () {
						const target = options.path;
						const replacement = text_encoder.encode(options.content);

						yield* file_system.makeDirectory(path_service.dirname(target), {
							mode: 0o700,
							recursive: true,
						});
						yield* file_system.makeDirectory(options.backups_directory, {
							mode: 0o700,
							recursive: true,
						});
						yield* permissions.RestrictDirectory(options.backups_directory).pipe(
							Effect.mapError(
								(cause) =>
									new ConfigFileBackupError({
										cause,
										path: options.backups_directory,
									}),
							),
						);

						const original = yield* ReadSnapshot(file_system, target);
						const recovery_backup =
							options.expected_content_hash === undefined
								? undefined
								: yield* FindRecoveryBackup(
										file_system,
										path_service,
										options.backups_directory,
										options.backup_name,
										options.expected_content_hash,
									);

						if (
							(options.expected_content_hash === undefined &&
								original !== undefined) ||
							(options.expected_content_hash !== undefined &&
								original !== undefined &&
								original.content_hash !== options.expected_content_hash)
						) {
							return changed(original, target);
						}

						const staged_path = path_service.join(
							options.backups_directory,
							`${options.backup_name}.replacement-${randomUUID()}`,
						);
						const staged = yield* WritePrivateExclusive(
							file_system,
							permissions,
							staged_path,
							replacement,
							hooks.after_private_truncate,
						);

						if (staged._tag === "Occupied") {
							return yield* new ConfigFileWriteError({
								cause: new Error(
									"The private staged replacement path was already occupied",
								),
								path: staged_path,
							});
						}

						if (staged._tag === "WriteFailed") {
							return yield* staged.cause;
						}

						let backup_path: string | undefined;
						let source = original;
						let source_permissions: PrivateFilePermissionsSnapshot | undefined;

						if (original !== undefined) {
							yield* RunHook(hooks.before_backup, target);
							yield* RunHook(hooks.before_compare, target);

							const observed = yield* ReadSnapshot(file_system, target);

							if (observed?.content_hash !== original.content_hash) {
								return changed(observed, target);
							}

							backup_path = path_service.join(
								options.backups_directory,
								`${options.backup_name}.original-${randomUUID()}`,
							);
							source_permissions = yield* permissions.Capture(target).pipe(
								Effect.mapError(
									(cause) =>
										new ConfigFileBackupError({
											cause,
											path: target,
										}),
								),
							);
							yield* WritePermissionsSnapshot(
								file_system,
								permissions,
								`${backup_path}.permissions.json`,
								source_permissions,
							);

							const moved = yield* MoveToBackup(
								file_system,
								target,
								backup_path,
								original,
							);

							if (moved._tag === "Changed") {
								return changed(
									yield* ReadSnapshot(file_system, target),
									target,
									backup_path,
								);
							}

							source = moved.snapshot;
							yield* RestrictBackupOrRestore({
								backup: moved.snapshot,
								backup_path,
								file_system,
								hooks,
								permissions,
								permissions_snapshot: source_permissions,
								target,
							});
						} else if (
							options.expected_content_hash !== undefined &&
							recovery_backup?.snapshot.content_hash === options.expected_content_hash
						) {
							backup_path = recovery_backup.path;
							source = recovery_backup.snapshot;
							source_permissions = recovery_backup.permissions_snapshot;
							yield* RestrictBackupOrRestore({
								backup: recovery_backup.snapshot,
								backup_path: recovery_backup.path,
								file_system,
								hooks,
								permissions,
								permissions_snapshot: recovery_backup.permissions_snapshot,
								target,
							});
						} else if (options.expected_content_hash !== undefined) {
							return changed(undefined, target);
						}

						yield* RunHook(hooks.before_replace, target);
						yield* RunHook(hooks.after_claim, target);

						const linked = yield* file_system.link(staged_path, target).pipe(
							Effect.as(true),
							Effect.catch((cause) =>
								is_platform_reason(cause, "AlreadyExists")
									? Effect.succeed(false)
									: Effect.fail(
											new ConfigFileWriteError({
												cause,
												path: target,
											}),
										),
							),
						);

						if (!linked) {
							return changed(
								yield* ReadSnapshot(file_system, target),
								target,
								backup_path,
							);
						}

						const rollback_backup =
							backup_path === undefined ||
							source === undefined ||
							source_permissions === undefined
								? undefined
								: {
										path: backup_path,
										permissions_snapshot: source_permissions,
										snapshot: source,
									};

						const written_identity = staged.identity;
						const target_permissions = yield* ApplyTargetPermissions({
							file_system,
							hooks,
							identity: written_identity,
							permissions,
							...(source_permissions === undefined
								? {}
								: { permissions_snapshot: source_permissions }),
							target,
						}).pipe(Effect.result);

						if (Result.isFailure(target_permissions)) {
							yield* RollbackPublication({
								...(rollback_backup === undefined
									? {}
									: { backup: rollback_backup }),
								file_system,
								hooks,
								permissions,
								replacement_identity: written_identity,
								target,
							});

							return yield* new ConfigFileReplaceError({
								cause: target_permissions.failure,
								path: target,
							});
						}

						if (!target_permissions.success) {
							return changed(
								yield* ReadSnapshot(file_system, target),
								target,
								backup_path,
							);
						}

						const after_replace = yield* RunHook(hooks.after_replace, target).pipe(
							Effect.exit,
						);

						if (Exit.isFailure(after_replace)) {
							yield* RollbackPublication({
								...(rollback_backup === undefined
									? {}
									: { backup: rollback_backup }),
								file_system,
								hooks,
								permissions,
								replacement_identity: written_identity,
								target,
							});

							return yield* new ConfigFileReplaceError({
								cause: Cause.squash(after_replace.cause),
								path: target,
							});
						}

						const published = yield* ReadSnapshot(file_system, target);

						if (published === undefined) {
							return changed(undefined, target, backup_path);
						}

						return {
							_tag: "Written" as const,
							...published,
							...(backup_path === undefined ? {} : { backup_path }),
						};
					}).pipe(
						Effect.mapError((cause) => {
							if (
								cause instanceof ConfigFileBackupError ||
								cause instanceof ConfigFileReplaceError ||
								cause instanceof ConfigFileRestoreError ||
								cause instanceof ConfigFileWriteError
							) {
								return cause;
							}

							return new ConfigFileReplaceError({
								cause,
								path: options.path,
							});
						}),
					),
			};
		}),
	);
}

/** Builds the Node-backed config-file service for desktop composition and integration tests. */
export function make_config_file_store_layer(hooks: ConfigFileHooks = {}) {
	const node_platform = NodeChildProcessSpawner.layer.pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
	);
	const platform = Layer.succeed(PrivateFilePermissionsPlatform, {
		kind: process.platform === "win32" ? "win32" : "posix",
	});
	const permissions = make_private_file_permissions_layer.pipe(Layer.provide(platform));

	return make_config_file_store_platform_layer(hooks).pipe(
		Layer.provide(permissions),
		Layer.provide(node_platform),
	);
}

/** Provides the default live Node config-file service. */
export const ConfigFileStoreLive = make_config_file_store_layer();
