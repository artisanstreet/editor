import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Exit, type FileSystem as FileSystemTypes } from "effect";
import { FileSystem } from "effect/FileSystem";
import { afterEach, describe, expect, it } from "vitest";

import { ReadFileIdentity } from "../../modules/backend/src/filesystem/file-identity";
import {
	FileDescriptorOf,
	FileDescriptorUnavailable,
} from "../../modules/backend/src/filesystem/node/file-descriptor";

const directories: Array<string> = [];

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((path) => rm(path, { force: true, recursive: true })),
	);
});

describe("opened file descriptor access", () => {
	/**
	 * Effect declared `fd` on the portable `File` interface through beta.100 and
	 * removed it in beta.103, while the Node implementation kept carrying it.
	 * This module is the single place that knows that, so it is the place to
	 * prove it — and the assertion is on the identity, not merely the number,
	 * because the descriptor exists to make the identity exact.
	 */
	it("reads the descriptor of a Node-opened file and identifies it exactly", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-fd-"));
		directories.push(directory);
		const path = join(directory, "private.json");

		const identity = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const file_system = yield* FileSystem;
					const file = yield* file_system.open(path, { flag: "w+" });
					const descriptor = yield* FileDescriptorOf(file, "test identity");

					expect(typeof descriptor).toBe("number");

					return yield* ReadFileIdentity(descriptor);
				}),
			).pipe(Effect.provide(NodeFileSystem.layer), Effect.orDie),
		);

		/**
		 * Bigint, not number: a Windows NTFS file id above 2^53 loses precision
		 * as a double, which is exactly how two different files come to compare
		 * equal. `file.stat` reports these as numbers, so it is not a substitute.
		 */
		expect(typeof identity.device).toBe("bigint");
		expect(typeof identity.inode).toBe("bigint");
	});

	/**
	 * A file system with no descriptor to give must fail loudly. Every caller
	 * uses the descriptor to prove a file's identity before writing to it, so
	 * proceeding without one would silently downgrade a defended replacement
	 * race into an undefended write.
	 */
	it("fails rather than proceed when a file exposes no descriptor", async () => {
		const outcome = await Effect.runPromise(
			Effect.exit(FileDescriptorOf({} as FileSystemTypes.File, "identity check")),
		);

		expect(Exit.isFailure(outcome)).toBe(true);
		expect(String(Exit.isFailure(outcome) ? outcome.cause : "")).toContain(
			"FileDescriptorUnavailable",
		);
		expect(new FileDescriptorUnavailable({ operation: "identity check" }).message).toContain(
			"identity check",
		);
	});
});
