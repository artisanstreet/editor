import { randomBytes } from "node:crypto";
import { mkdir, rename, rm, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import { Effect } from "effect";

/**
 * Writes through a same-directory replacement so readers never observe partial
 * content: unique temporary sibling, then an atomic rename over the target.
 * The single canonical implementation for every durable Forge file.
 */
export const WriteFileAtomically = (path: string, content: string) =>
	Effect.gen(function* () {
		yield* Effect.tryPromise(() => mkdir(dirname(path), { recursive: true }));
		const temporary = `${path}.${randomBytes(6).toString("hex")}.tmp`;
		yield* Effect.tryPromise(() =>
			writeFile(temporary, content, { encoding: "utf8", mode: 0o600 }),
		);
		yield* Effect.tryPromise(() => rename(temporary, path)).pipe(
			Effect.ensuring(
				Effect.tryPromise(() => rm(temporary, { force: true })).pipe(Effect.ignore),
			),
		);
	});
