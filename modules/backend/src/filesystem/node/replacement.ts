import { createHash } from "node:crypto";
import { fchmod } from "node:fs";

import { Cause, Effect, Exit, Layer, Option } from "effect";

import {
	BoundedRegularFileStoreError,
	type ReplaceRegularFileOptions,
	type ReplaceRegularFileResult,
} from "../bounded-regular-file-store";
import { ReadFileIdentity, same_file_identity, type FileIdentity } from "../file-identity";
import { FileDescriptorOf } from "./file-descriptor";
import {
	NodeReplacementContext,
	type NodeReplacementContextService,
	type RegularFileSnapshot,
} from "./context";

const conditional_backup_suffix_pattern =
	/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function make_store_error(
	operation: BoundedRegularFileStoreError["operation"],
	path: string,
	cause: unknown,
) {
	return new BoundedRegularFileStoreError({ cause, operation, path });
}

function is_missing_error(cause: unknown) {
	return (
		typeof cause === "object" &&
		cause !== null &&
		(("code" in cause && (cause.code === "ENOENT" || cause.code === "ENOTDIR")) ||
			("reason" in cause &&
				typeof cause.reason === "object" &&
				cause.reason !== null &&
				"_tag" in cause.reason &&
				cause.reason._tag === "NotFound"))
	);
}

function is_already_exists_error(cause: unknown) {
	return (
		typeof cause === "object" &&
		cause !== null &&
		(("code" in cause && cause.code === "EEXIST") ||
			("reason" in cause &&
				typeof cause.reason === "object" &&
				cause.reason !== null &&
				"_tag" in cause.reason &&
				cause.reason._tag === "AlreadyExists"))
	);
}

function same_bytes(left: Uint8Array, right: Uint8Array) {
	return left.length === right.length && left.every((value, index) => value === right[index]);
}

function is_replace_regular_file_options(input: unknown): input is ReplaceRegularFileOptions {
	return (
		typeof input === "object" &&
		input !== null &&
		"expected" in input &&
		input.expected instanceof Uint8Array &&
		"maximum_bytes" in input &&
		typeof input.maximum_bytes === "number" &&
		Number.isSafeInteger(input.maximum_bytes) &&
		input.maximum_bytes > 0 &&
		input.expected.length <= input.maximum_bytes &&
		"operation_id" in input &&
		typeof input.operation_id === "string" &&
		input.operation_id.length > 0 &&
		"path" in input &&
		typeof input.path === "string" &&
		input.path.length > 0 &&
		"replacement" in input &&
		input.replacement instanceof Uint8Array &&
		input.replacement.length <= input.maximum_bytes
	);
}

function RunHook(hook: ((path: string) => Promise<void>) | undefined, path: string) {
	return hook === undefined
		? Effect.void
		: Effect.tryPromise({
				try: () => hook(path),
				catch: (cause) => cause,
			});
}

function Fchmod(descriptor: number, mode: number) {
	return Effect.callback<void, Error>((resume) => {
		fchmod(descriptor, mode, (cause) => {
			resume(cause === null ? Effect.void : Effect.fail(cause));
		});
	});
}

export const MakeNodeReplacementService = Effect.gen(function* () {
	const {
		crypto,
		file_system,
		hooks,
		path_service,
		ReadOptionalRegularSnapshot,
		ReadRegularSnapshot,
		resolve_mutable_path,
		root,
	} = yield* NodeReplacementContext;
	const map_store_error = (operation: BoundedRegularFileStoreError["operation"], path: string) =>
		Effect.mapError((cause: unknown) => make_store_error(operation, path, cause));
	const RandomUuid = (_operation: "write", _path: string) => crypto.randomUUIDv4;
	const RemoveArtifact = (path: string) =>
		file_system.remove(path).pipe(
			Effect.as(true),
			Effect.catch((cause) =>
				is_missing_error(cause) ? Effect.succeed(false) : Effect.fail(cause),
			),
		);
	/** Windows exposes no directory-fsync operation through Effect or Node. */
	const SyncDirectory = (path: string) =>
		process.platform === "win32"
			? Effect.void
			: Effect.scoped(
					file_system
						.open(path, { flag: "r" })
						.pipe(Effect.flatMap((directory) => directory.sync)),
				);
	const LinkIfAbsent = (source: string, target: string) =>
		file_system.link(source, target).pipe(
			Effect.as(true),
			Effect.catch((cause) =>
				is_already_exists_error(cause) ? Effect.succeed(false) : Effect.fail(cause),
			),
		);
	const make_artifact_namespace = (operation_id: string, path: string) =>
		createHash("sha256").update(operation_id).update("\0").update(path).digest("hex");

	const ResolveReplacementPaths = (input: ReplaceRegularFileOptions) =>
		Effect.gen(function* () {
			const target = yield* resolve_mutable_path(input.path, "write");
			const relative = path_service.relative(root, target).replaceAll("\\", "/");
			const namespace_path = process.platform === "win32" ? relative.toLowerCase() : relative;
			const namespace = make_artifact_namespace(input.operation_id, namespace_path);
			const directory = path_service.dirname(target);

			return {
				backup_prefix: `.artisan-conditional-${namespace}.backup-`,
				directory,
				stage: path_service.join(directory, `.artisan-conditional-${namespace}.stage`),
				target,
			};
		});
	const ReadBackupPaths = (directory: string, backup_prefix: string) =>
		file_system
			.readDirectory(directory)
			.pipe(
				Effect.map((entries) =>
					entries
						.filter(
							(entry) =>
								entry.startsWith(backup_prefix) &&
								conditional_backup_suffix_pattern.test(
									entry.slice(backup_prefix.length),
								),
						)
						.map((entry) => path_service.join(directory, entry)),
				),
			);
	const CleanupArtifacts = (directory: string, stage: string, backup?: string) =>
		Effect.gen(function* () {
			const backup_removed = backup === undefined ? false : yield* RemoveArtifact(backup);

			if (backup_removed) {
				yield* SyncDirectory(directory);
			}

			const stage_removed = yield* RemoveArtifact(stage);

			if (stage_removed) {
				yield* SyncDirectory(directory);
			}
		});
	const ReplaceRegularFile = (input: ReplaceRegularFileOptions) => {
		const reported_path =
			typeof input === "object" &&
			input !== null &&
			"path" in input &&
			typeof input.path === "string"
				? input.path
				: ".";

		return Effect.suspend(() => {
			if (!is_replace_regular_file_options(input)) {
				return Effect.fail(new Error("invalid conditional replacement input"));
			}

			return Effect.gen(function* () {
				const paths = yield* ResolveReplacementPaths(input);
				const StageSnapshot = () =>
					ReadRegularSnapshot(paths.stage, input.path, input.maximum_bytes);
				const TargetSnapshot = () =>
					ReadOptionalRegularSnapshot(paths.target, input.path, input.maximum_bytes);
				const RetryState = (attempt: number) =>
					attempt >= 20
						? Effect.fail(new Error("conditional replacement did not converge"))
						: Effect.sleep("5 millis").pipe(Effect.andThen(Run(attempt + 1)));
				const Publish = (backup: string, staged: RegularFileSnapshot, attempt: number) =>
					Effect.gen(function* () {
						yield* RunHook(hooks?.before_publication, input.path);
						const linked = yield* LinkIfAbsent(paths.stage, paths.target);

						if (!linked) {
							const current = yield* TargetSnapshot();

							if (
								Option.isSome(current) &&
								same_bytes(current.value.bytes, input.replacement) &&
								same_file_identity(current.value.identity, staged.identity)
							) {
								return {
									_tag: "AlreadyReplaced",
								} satisfies ReplaceRegularFileResult;
							}

							yield* CleanupArtifacts(paths.directory, paths.stage, backup);

							return { _tag: "Changed" } satisfies ReplaceRegularFileResult;
						}

						yield* SyncDirectory(paths.directory);
						yield* RunHook(hooks?.after_publication, input.path);
						const published = yield* TargetSnapshot();
						const current_stage_exit = yield* StageSnapshot().pipe(Effect.exit);

						if (Option.isNone(published) || Exit.isFailure(current_stage_exit)) {
							return yield* RetryState(attempt);
						}

						const current_stage = current_stage_exit.value;

						if (
							!same_bytes(published.value.bytes, input.replacement) ||
							!same_file_identity(published.value.identity, current_stage.identity) ||
							!same_file_identity(current_stage.identity, staged.identity)
						) {
							return yield* Effect.fail(
								new Error("published replacement changed before acknowledgement"),
							);
						}

						return { _tag: "Replaced" } satisfies ReplaceRegularFileResult;
					});
				const Run: (attempt: number) => Effect.Effect<ReplaceRegularFileResult, unknown> = (
					attempt,
				) =>
					Effect.gen(function* () {
						const stage_exit = yield* ReadOptionalRegularSnapshot(
							paths.stage,
							input.path,
							input.maximum_bytes,
						).pipe(Effect.exit);

						if (Exit.isFailure(stage_exit)) {
							return attempt < 20
								? yield* RetryState(attempt)
								: yield* Effect.fail(Cause.squash(stage_exit.cause));
						}

						const stage = stage_exit.value;
						const backups = yield* ReadBackupPaths(
							paths.directory,
							paths.backup_prefix,
						);

						if (backups.length > 1) {
							return attempt < 20
								? yield* RetryState(attempt)
								: yield* Effect.fail(
										new Error("ambiguous conditional replacement backups"),
									);
						}

						const backup = backups[0];
						const captured_option =
							backup === undefined
								? Option.none<RegularFileSnapshot>()
								: yield* ReadOptionalRegularSnapshot(
										backup,
										input.path,
										input.maximum_bytes,
									);

						if (backup !== undefined && Option.isNone(captured_option)) {
							return yield* RetryState(attempt);
						}

						const captured = Option.getOrUndefined(captured_option);
						const current = yield* TargetSnapshot();
						const CapturedBackupPath = () =>
							backup === undefined
								? Effect.fail(
										new Error("conditional replacement backup path is missing"),
									)
								: Effect.succeed(backup);

						if (captured !== undefined && !same_bytes(captured.bytes, input.expected)) {
							return yield* Effect.fail(
								new Error("conditional replacement backup is corrupt"),
							);
						}

						if (Option.isSome(stage)) {
							if (!same_bytes(stage.value.bytes, input.replacement)) {
								return yield* Effect.fail(
									new Error("conditional replacement stage is corrupt"),
								);
							}

							const current_snapshot = Option.getOrUndefined(current);
							const expected_mode =
								captured?.mode ??
								(current_snapshot !== undefined &&
								same_bytes(current_snapshot.bytes, input.expected)
									? current_snapshot.mode
									: undefined);

							if (expected_mode !== undefined && stage.value.mode !== expected_mode) {
								return yield* Effect.fail(
									new Error("conditional replacement stage mode is corrupt"),
								);
							}
						}

						if (Option.isNone(stage) && captured !== undefined) {
							const captured_backup = yield* CapturedBackupPath();

							if (Option.isNone(current)) {
								const restored = yield* LinkIfAbsent(captured_backup, paths.target);

								if (!restored) {
									return yield* RetryState(attempt);
								}

								yield* SyncDirectory(paths.directory);
							}

							yield* CleanupArtifacts(paths.directory, paths.stage, backup);

							return { _tag: "Changed" } satisfies ReplaceRegularFileResult;
						}

						if (Option.isSome(stage) && captured !== undefined) {
							const captured_backup = yield* CapturedBackupPath();

							if (Option.isNone(current)) {
								return yield* Publish(captured_backup, stage.value, attempt);
							}

							if (
								same_bytes(current.value.bytes, input.replacement) &&
								same_file_identity(current.value.identity, stage.value.identity)
							) {
								return {
									_tag: "AlreadyReplaced",
								} satisfies ReplaceRegularFileResult;
							}

							yield* CleanupArtifacts(paths.directory, paths.stage, backup);

							return { _tag: "Changed" } satisfies ReplaceRegularFileResult;
						}

						if (Option.isSome(stage)) {
							if (
								Option.isNone(current) ||
								!same_bytes(current.value.bytes, input.expected)
							) {
								yield* CleanupArtifacts(paths.directory, paths.stage);

								return { _tag: "Changed" } satisfies ReplaceRegularFileResult;
							}
						} else {
							if (
								Option.isNone(current) ||
								!same_bytes(current.value.bytes, input.expected)
							) {
								return { _tag: "Changed" } satisfies ReplaceRegularFileResult;
							}

							if (same_bytes(input.expected, input.replacement)) {
								return {
									_tag: "AlreadyReplaced",
								} satisfies ReplaceRegularFileResult;
							}

							const created = yield* Effect.scoped(
								file_system
									.open(paths.stage, {
										flag: "wx",
										mode: current.value.mode,
									})
									.pipe(
										Effect.flatMap((file) =>
											Effect.gen(function* () {
												const descriptor = yield* FileDescriptorOf(
													file,
													"staged file identity",
												);
												const identity =
													yield* ReadFileIdentity(descriptor);

												yield* file.writeAll(input.replacement);
												yield* Fchmod(descriptor, current.value.mode);
												yield* file.sync;

												return Option.some(identity);
											}),
										),
										Effect.catch((cause) =>
											is_already_exists_error(cause)
												? Effect.succeed(Option.none<FileIdentity>())
												: Effect.fail(cause),
										),
									),
							);

							if (Option.isNone(created)) {
								return yield* RetryState(attempt);
							}

							yield* SyncDirectory(paths.directory);
							const written = yield* StageSnapshot();

							if (
								!same_bytes(written.bytes, input.replacement) ||
								written.mode !== current.value.mode ||
								!same_file_identity(written.identity, created.value)
							) {
								return yield* Effect.fail(
									new Error(
										"conditional replacement stage changed while writing",
									),
								);
							}

							yield* RunHook(hooks?.after_stage, input.path);
						}

						const staged = yield* StageSnapshot();
						const compared = yield* TargetSnapshot();

						if (
							Option.isNone(compared) ||
							!same_bytes(compared.value.bytes, input.expected) ||
							compared.value.mode !== staged.mode
						) {
							yield* CleanupArtifacts(paths.directory, paths.stage);

							return { _tag: "Changed" } satisfies ReplaceRegularFileResult;
						}

						yield* RunHook(hooks?.before_backup, input.path);
						const ready = yield* TargetSnapshot();

						if (
							Option.isNone(ready) ||
							!same_bytes(ready.value.bytes, input.expected) ||
							ready.value.mode !== compared.value.mode ||
							!same_file_identity(ready.value.identity, compared.value.identity)
						) {
							return yield* RetryState(attempt);
						}

						const created_backup = path_service.join(
							paths.directory,
							`${paths.backup_prefix}${yield* RandomUuid("write", input.path)}`,
						);
						const moved = yield* file_system.rename(paths.target, created_backup).pipe(
							Effect.as(true),
							Effect.catch((cause) =>
								is_missing_error(cause)
									? Effect.succeed(false)
									: Effect.fail(cause),
							),
						);

						if (!moved) {
							return yield* RetryState(attempt);
						}

						yield* SyncDirectory(paths.directory);
						const moved_option = yield* ReadOptionalRegularSnapshot(
							created_backup,
							input.path,
							input.maximum_bytes,
						);

						if (Option.isNone(moved_option)) {
							return yield* RetryState(attempt);
						}

						const moved_snapshot = moved_option.value;

						if (
							same_bytes(moved_snapshot.bytes, input.replacement) &&
							same_file_identity(moved_snapshot.identity, staged.identity)
						) {
							const restored = yield* LinkIfAbsent(created_backup, paths.target);

							if (!restored) {
								return yield* Effect.fail(
									new Error(
										"published replacement could not be restored without overwrite",
									),
								);
							}

							yield* SyncDirectory(paths.directory);
							yield* RemoveArtifact(created_backup);
							yield* SyncDirectory(paths.directory);

							return {
								_tag: "AlreadyReplaced",
							} satisfies ReplaceRegularFileResult;
						}

						if (
							!same_bytes(moved_snapshot.bytes, input.expected) ||
							moved_snapshot.mode !== compared.value.mode ||
							!same_file_identity(moved_snapshot.identity, compared.value.identity)
						) {
							const restored = yield* LinkIfAbsent(created_backup, paths.target);

							if (!restored) {
								return yield* Effect.fail(
									new Error(
										"raced target could not be restored without overwrite",
									),
								);
							}

							yield* SyncDirectory(paths.directory);
							yield* CleanupArtifacts(paths.directory, paths.stage, created_backup);

							return { _tag: "Changed" } satisfies ReplaceRegularFileResult;
						}

						yield* RunHook(hooks?.after_backup, input.path);

						return yield* Publish(created_backup, staged, attempt);
					});

				return yield* Run(0);
			});
		}).pipe(map_store_error("replace", reported_path));
	};
	const FinalizeRegularFileReplacement = (input: ReplaceRegularFileOptions) => {
		const reported_path =
			typeof input === "object" &&
			input !== null &&
			"path" in input &&
			typeof input.path === "string"
				? input.path
				: ".";

		return Effect.suspend(() => {
			if (!is_replace_regular_file_options(input)) {
				return Effect.fail(new Error("invalid conditional replacement input"));
			}

			return Effect.gen(function* () {
				const paths = yield* ResolveReplacementPaths(input);
				const backups = yield* ReadBackupPaths(paths.directory, paths.backup_prefix);

				if (backups.length > 1) {
					return yield* Effect.fail(
						new Error("ambiguous conditional replacement backups"),
					);
				}

				const stage = yield* ReadOptionalRegularSnapshot(
					paths.stage,
					input.path,
					input.maximum_bytes,
				);
				const backup =
					backups[0] === undefined
						? Option.none<RegularFileSnapshot>()
						: yield* ReadOptionalRegularSnapshot(
								backups[0],
								input.path,
								input.maximum_bytes,
							);

				if (Option.isNone(stage) && Option.isNone(backup)) {
					return;
				}

				if (Option.isNone(stage) && Option.isSome(backup)) {
					return yield* Effect.fail(
						new Error("conditional replacement receipt is incomplete"),
					);
				}

				const current = yield* ReadOptionalRegularSnapshot(
					paths.target,
					input.path,
					input.maximum_bytes,
				);

				if (
					Option.isNone(current) ||
					!same_bytes(current.value.bytes, input.replacement) ||
					(Option.isSome(stage) &&
						(!same_bytes(stage.value.bytes, input.replacement) ||
							!same_file_identity(current.value.identity, stage.value.identity))) ||
					(Option.isSome(backup) && !same_bytes(backup.value.bytes, input.expected)) ||
					(Option.isSome(stage) &&
						Option.isSome(backup) &&
						stage.value.mode !== backup.value.mode)
				) {
					return yield* Effect.fail(
						new Error("conditional replacement receipt is not finalizable"),
					);
				}

				const backup_removed =
					backups[0] === undefined ? false : yield* RemoveArtifact(backups[0]);

				if (backup_removed) {
					yield* SyncDirectory(paths.directory);
					yield* RunHook(hooks?.after_backup_cleanup, input.path);
				}

				const stage_removed = yield* RemoveArtifact(paths.stage);

				if (stage_removed) {
					yield* SyncDirectory(paths.directory);
					yield* RunHook(hooks?.after_stage_cleanup, input.path);
				}
			});
		}).pipe(map_store_error("finalize", reported_path));
	};
	return { FinalizeRegularFileReplacement, ReplaceRegularFile };
});

export const make_node_replacement_context_layer = (context: NodeReplacementContextService) =>
	Layer.succeed(NodeReplacementContext, context);
