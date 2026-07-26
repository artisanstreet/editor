import { createHash, randomUUID } from "node:crypto";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Context, Data, Effect, FileSystem, Layer, Option, Path } from "effect";

import { normalize_global_guidance_content } from "@artisan/protocol";

/** Contains one file value and its last-modified timestamp. */
export interface GuidanceFile {
	readonly content: string;
	readonly modified_at: string;
}

/** Represents an isolated filesystem boundary failure. */
export class GuidanceFileStoreFailure extends Data.TaggedError("GuidanceFileStoreFailure")<{
	readonly backup_path?: string;
	readonly cause: unknown;
	readonly operation: "backup" | "read" | "replace" | "restore" | "write";
	readonly path: string;
}> {}

/** Supplies an observed-state fence for a recoverable provider-file replacement. */
export interface GuidanceConditionalWriteInput {
	readonly backup_name: string;
	readonly backups_directory: string;
	readonly content: string;
	readonly expected_hash?: string;
	readonly path: string;
}

/** Reports whether a provider file was replaced or changed before publication. */
export type GuidanceConditionalWriteResult =
	| {
			readonly _tag: "Changed";
			readonly backup_path?: string;
	  }
	| {
			readonly _tag: "Written";
			readonly backup_path?: string;
	  };

/** Owns atomic canonical/mirror writes and recoverable backup creation. */
export class GuidanceFileStore extends Context.Service<
	GuidanceFileStore,
	{
		readonly CopyToBackup: (
			source_path: string,
			backups_directory: string,
			backup_name: string,
		) => Effect.Effect<string, GuidanceFileStoreFailure>;
		readonly Read: (
			path: string,
		) => Effect.Effect<Option.Option<GuidanceFile>, GuidanceFileStoreFailure>;
		readonly ReplaceAtomic: (
			input: GuidanceConditionalWriteInput,
		) => Effect.Effect<GuidanceConditionalWriteResult, GuidanceFileStoreFailure>;
		readonly WriteAtomic: (
			path: string,
			content: string,
		) => Effect.Effect<void, GuidanceFileStoreFailure>;
	}
>()("Artisan/GuidanceFileStore") {}

const failure = (
	path: string,
	cause: unknown,
	operation: GuidanceFileStoreFailure["operation"],
	backup_path?: string,
) =>
	new GuidanceFileStoreFailure({
		...(backup_path === undefined ? {} : { backup_path }),
		cause,
		operation,
		path,
	});

function is_platform_reason(cause: unknown, reason: "AlreadyExists" | "NotFound") {
	return (
		typeof cause === "object" &&
		cause !== null &&
		"reason" in cause &&
		typeof cause.reason === "object" &&
		cause.reason !== null &&
		"_tag" in cause.reason &&
		cause.reason._tag === reason
	);
}

function content_hash(content: string) {
	return createHash("sha256").update(normalize_global_guidance_content(content)).digest("hex");
}

const RemoveTemporary = (file_system: FileSystem.FileSystem, temporary_path: string) =>
	file_system.remove(temporary_path).pipe(Effect.ignore);

const PrepareFile = (
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	path: string,
	content: string,
) =>
	Effect.gen(function* () {
		const directory = path_service.dirname(path);
		const temporary_path = path_service.join(
			directory,
			`.${path_service.basename(path)}.artisan-write-${randomUUID()}`,
		);

		yield* file_system.makeDirectory(directory, { recursive: true });
		yield* Effect.scoped(
			Effect.gen(function* () {
				const file = yield* file_system.open(temporary_path, { flag: "wx" });
				const bytes = new TextEncoder().encode(content);

				if (bytes.byteLength > 0) {
					yield* file.writeAll(bytes);
				}

				yield* file.sync;
			}),
		);

		return temporary_path;
	});

const PublishPreparedIfAbsent = (
	file_system: FileSystem.FileSystem,
	temporary_path: string,
	path: string,
) =>
	file_system.link(temporary_path, path).pipe(
		Effect.as(true),
		Effect.catch((cause) =>
			is_platform_reason(cause, "AlreadyExists") ? Effect.succeed(false) : Effect.fail(cause),
		),
	);

const PublishContentIfAbsent = (
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	path: string,
	content: string,
) =>
	Effect.gen(function* () {
		const temporary_path = yield* PrepareFile(file_system, path_service, path, content);

		return yield* PublishPreparedIfAbsent(file_system, temporary_path, path).pipe(
			Effect.ensuring(RemoveTemporary(file_system, temporary_path)),
		);
	});

const RestoreBackupIfAbsent = (
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	path: string,
	backup_path: string,
) =>
	Effect.gen(function* () {
		const previous_content = yield* file_system.readFileString(backup_path);
		yield* PublishContentIfAbsent(file_system, path_service, path, previous_content);
	});

/** Supplies the production Node implementation behind the narrow file-store service. */
const GuidanceFileStorePlatformLive = Layer.effect(
	GuidanceFileStore,
	Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const path_service = yield* Path.Path;

		return {
			CopyToBackup: (source_path, backups_directory, backup_name) =>
				Effect.gen(function* () {
					const content = yield* file_system.readFileString(source_path);
					const backup_path = path_service.join(backups_directory, backup_name);

					yield* file_system.makeDirectory(backups_directory, { recursive: true });
					yield* Effect.scoped(
						Effect.gen(function* () {
							const file = yield* file_system.open(backup_path, { flag: "wx" });
							const bytes = new TextEncoder().encode(content);

							if (bytes.byteLength > 0) {
								yield* file.writeAll(bytes);
							}
						}),
					);

					return backup_path;
				}).pipe(Effect.mapError((cause) => failure(source_path, cause, "backup"))),
			Read: (path) =>
				Effect.all([file_system.readFileString(path), file_system.stat(path)], {
					concurrency: "unbounded",
				}).pipe(
					Effect.map(([content, metadata]) =>
						Option.some({
							content,
							modified_at: Option.getOrElse(
								metadata.mtime,
								() => new Date(0),
							).toISOString(),
						}),
					),
					Effect.catch((cause) =>
						is_platform_reason(cause, "NotFound")
							? Effect.succeed(Option.none())
							: Effect.fail(failure(path, cause, "read")),
					),
				),
			ReplaceAtomic: (input) =>
				Effect.gen(function* () {
					const temporary_path = yield* PrepareFile(
						file_system,
						path_service,
						input.path,
						input.content,
					);

					return yield* Effect.gen(function* () {
						if (input.expected_hash === undefined) {
							const published = yield* PublishPreparedIfAbsent(
								file_system,
								temporary_path,
								input.path,
							);

							return { _tag: published ? "Written" : "Changed" } as const;
						}

						yield* file_system.makeDirectory(input.backups_directory, {
							recursive: true,
						});
						const backup_path = path_service.join(
							input.backups_directory,
							input.backup_name,
						);
						const moved = yield* file_system.rename(input.path, backup_path).pipe(
							Effect.as(true),
							Effect.catch((cause) =>
								is_platform_reason(cause, "NotFound")
									? Effect.succeed(false)
									: Effect.fail(cause),
							),
						);

						if (!moved) {
							return { _tag: "Changed" } as const;
						}

						const ReplaceAfterBackup = Effect.gen(function* () {
							const previous_content = yield* file_system.readFileString(backup_path);

							if (content_hash(previous_content) !== input.expected_hash) {
								yield* RestoreBackupIfAbsent(
									file_system,
									path_service,
									input.path,
									backup_path,
								).pipe(
									Effect.mapError((cause) =>
										failure(input.path, cause, "restore", backup_path),
									),
								);

								return { _tag: "Changed", backup_path } as const;
							}

							const published = yield* PublishPreparedIfAbsent(
								file_system,
								temporary_path,
								input.path,
							);

							return {
								_tag: published ? "Written" : "Changed",
								backup_path,
							} as const;
						});

						return yield* ReplaceAfterBackup.pipe(
							Effect.catch((cause) =>
								RestoreBackupIfAbsent(
									file_system,
									path_service,
									input.path,
									backup_path,
								).pipe(
									Effect.mapError((restore_cause) =>
										failure(input.path, restore_cause, "restore", backup_path),
									),
									Effect.flatMap(() =>
										Effect.fail(
											cause instanceof GuidanceFileStoreFailure
												? cause
												: failure(
														input.path,
														cause,
														"replace",
														backup_path,
													),
										),
									),
								),
							),
						);
					}).pipe(Effect.ensuring(RemoveTemporary(file_system, temporary_path)));
				}).pipe(
					Effect.mapError((cause) =>
						cause instanceof GuidanceFileStoreFailure
							? cause
							: failure(input.path, cause, "replace"),
					),
				),
			WriteAtomic: (path, content) =>
				Effect.gen(function* () {
					const temporary_path = yield* PrepareFile(
						file_system,
						path_service,
						path,
						content,
					);
					yield* file_system
						.rename(temporary_path, path)
						.pipe(Effect.ensuring(RemoveTemporary(file_system, temporary_path)));
				}).pipe(Effect.mapError((cause) => failure(path, cause, "write"))),
		};
	}),
);

/** Supplies the production Node implementation behind the narrow file-store service. */
export const GuidanceFileStoreLive = GuidanceFileStorePlatformLive.pipe(
	Layer.provide(NodeFileSystem.layer),
	Layer.provide(NodePath.layer),
);
