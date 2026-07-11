import { createHash, randomUUID } from "node:crypto";
import { constants as fs_constants } from "node:fs";
import {
	copyFile,
	link,
	mkdir,
	open,
	readFile,
	rename,
	stat,
	unlink,
	writeFile,
} from "node:fs/promises";
import { basename, dirname, join } from "node:path";

import { Context, Data, Effect, Layer, Option } from "effect";

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

function is_error_code(cause: unknown, code: string) {
	return (cause as NodeJS.ErrnoException).code === code;
}

function content_hash(content: string) {
	return createHash("sha256").update(normalize_global_guidance_content(content)).digest("hex");
}

async function prepare_file(path: string, content: string) {
	const directory = dirname(path);
	const temporary_path = join(directory, `.${basename(path)}.artisan-write-${randomUUID()}`);

	await mkdir(directory, { recursive: true });
	const handle = await open(temporary_path, "wx");

	try {
		await handle.writeFile(content, "utf8");
		await handle.sync();
	} finally {
		await handle.close();
	}

	return temporary_path;
}

async function publish_prepared_if_absent(temporary_path: string, path: string) {
	try {
		await link(temporary_path, path);

		return true;
	} catch (cause: unknown) {
		if (is_error_code(cause, "EEXIST")) {
			return false;
		}

		throw cause;
	}
}

async function publish_content_if_absent(path: string, content: string) {
	const temporary_path = await prepare_file(path, content);

	try {
		return await publish_prepared_if_absent(temporary_path, path);
	} finally {
		await unlink(temporary_path).catch(() => undefined);
	}
}

async function restore_backup_if_absent(path: string, backup_path: string) {
	const previous_content = await readFile(backup_path, "utf8");

	try {
		await publish_content_if_absent(path, previous_content);

		return;
	} catch {
		try {
			await copyFile(backup_path, path, fs_constants.COPYFILE_EXCL);
		} catch (copy_cause: unknown) {
			if (is_error_code(copy_cause, "EEXIST")) {
				return;
			}

			throw copy_cause;
		}
	}
}

/** Supplies the production Node implementation behind the narrow file-store service. */
export const GuidanceFileStoreLive = Layer.succeed(GuidanceFileStore, {
	CopyToBackup: (source_path, backups_directory, backup_name) =>
		Effect.tryPromise({
			catch: (cause) => failure(source_path, cause, "backup"),
			try: async () => {
				const content = await readFile(source_path, "utf8");
				const backup_path = join(backups_directory, backup_name);

				await mkdir(backups_directory, { recursive: true });
				await writeFile(backup_path, content, { encoding: "utf8", flag: "wx" });

				return backup_path;
			},
		}),
	Read: (path) =>
		Effect.tryPromise({
			catch: (cause) => failure(path, cause, "read"),
			try: async () => {
				try {
					const [content, metadata] = await Promise.all([
						readFile(path, "utf8"),
						stat(path),
					]);

					return Option.some({ content, modified_at: metadata.mtime.toISOString() });
				} catch (cause: unknown) {
					if ((cause as NodeJS.ErrnoException).code === "ENOENT") {
						return Option.none();
					}

					throw cause;
				}
			},
		}),
	ReplaceAtomic: (input) => {
		let backup_path: string | undefined;
		let operation: GuidanceFileStoreFailure["operation"] = "replace";

		return Effect.tryPromise({
			catch: (cause) => failure(input.path, cause, operation, backup_path),
			try: async () => {
				const temporary_path = await prepare_file(input.path, input.content);

				try {
					if (input.expected_hash === undefined) {
						const published = await publish_prepared_if_absent(
							temporary_path,
							input.path,
						);

						return { _tag: published ? "Written" : "Changed" } as const;
					}

					await mkdir(input.backups_directory, { recursive: true });
					backup_path = join(input.backups_directory, input.backup_name);

					try {
						await rename(input.path, backup_path);
					} catch (cause: unknown) {
						if (is_error_code(cause, "ENOENT")) {
							return { _tag: "Changed" } as const;
						}

						throw cause;
					}

					const previous_content = await readFile(backup_path, "utf8");

					if (content_hash(previous_content) !== input.expected_hash) {
						operation = "restore";
						await restore_backup_if_absent(input.path, backup_path);
						operation = "replace";

						return { _tag: "Changed", backup_path } as const;
					}

					const published = await publish_prepared_if_absent(temporary_path, input.path);

					return {
						_tag: published ? "Written" : "Changed",
						backup_path,
					} as const;
				} catch (cause: unknown) {
					const failed_operation = operation;

					if (backup_path !== undefined) {
						operation = "restore";
						await restore_backup_if_absent(input.path, backup_path);
						operation = failed_operation;
					}

					throw cause;
				} finally {
					await unlink(temporary_path).catch(() => undefined);
				}
			},
		});
	},
	WriteAtomic: (path, content) =>
		Effect.tryPromise({
			catch: (cause) => failure(path, cause, "write"),
			try: async () => {
				const temporary_path = await prepare_file(path, content);

				try {
					await rename(temporary_path, path);
				} finally {
					await unlink(temporary_path).catch(() => undefined);
				}
			},
		}),
});
