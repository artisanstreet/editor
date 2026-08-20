import { createHash } from "node:crypto";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { BoundedRegularFileStore } from "../../modules/backend/src/filesystem/bounded-regular-file-store";
import { Filesystem } from "../../modules/backend/src/filesystem/filesystem";
import {
	make_node_filesystem_layer,
	make_node_non_adversarial_bounded_regular_file_store_layer,
	type NodeBoundedRegularFileStoreHooks,
} from "../../modules/backend/src/filesystem/node-filesystem";

const roots: Array<string> = [];
const encoder = new TextEncoder();

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan conditional filesystem "));

	roots.push(root);

	return root;
}

async function make_filesystem(root: string, hooks: NodeBoundedRegularFileStoreHooks = {}) {
	return Effect.runPromise(
		Effect.service(BoundedRegularFileStore).pipe(
			Effect.provide(
				make_node_non_adversarial_bounded_regular_file_store_layer({ hooks, root }),
			),
		),
	);
}

async function make_observer(root: string) {
	return Effect.runPromise(
		Effect.service(Filesystem).pipe(Effect.provide(make_node_filesystem_layer({ root }))),
	);
}

function replace(
	filesystem: BoundedRegularFileStore["Service"],
	path: string,
	expected: string,
	replacement: string,
	operation_id = "replace-operation",
) {
	return filesystem.ReplaceRegularFile({
		expected: encoder.encode(expected),
		maximum_bytes: 1024,
		operation_id,
		path,
		replacement: encoder.encode(replacement),
	});
}

function finalize(
	filesystem: BoundedRegularFileStore["Service"],
	path: string,
	expected: string,
	replacement: string,
	operation_id = "replace-operation",
) {
	return filesystem.FinalizeRegularFileReplacement({
		expected: encoder.encode(expected),
		maximum_bytes: 1024,
		operation_id,
		path,
		replacement: encoder.encode(replacement),
	});
}

function artifact_namespace(operation_id: string, path: string) {
	return createHash("sha256").update(operation_id).update("\0").update(path).digest("hex");
}

function artifact_paths(root: string, operation_id: string, path: string) {
	const namespace = artifact_namespace(operation_id, path);

	return {
		stage: join(root, `.artisan-conditional-${namespace}.stage`),
		backup_prefix: `.artisan-conditional-${namespace}.backup-`,
	};
}

function make_barrier(expected_entries: number) {
	let entries = 0;
	let release!: () => void;
	const released = new Promise<void>((resolve) => {
		release = resolve;
	});

	return {
		enter: async () => {
			entries += 1;

			if (entries === expected_entries) release();

			await released;
		},
	};
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("BoundedRegularFileStore conditional replacement", () => {
	it("keeps conditional mutation outside the ordinary filesystem surface", async () => {
		const root = await make_root();
		const filesystem = await make_observer(root);
		const store = await make_filesystem(root);

		expect("ReadRegularFile" in filesystem).toBe(false);
		expect("ReplaceRegularFile" in filesystem).toBe(false);
		expect("FinalizeRegularFileReplacement" in filesystem).toBe(false);
		expect(Object.keys(store).sort()).toEqual([
			"FinalizeRegularFileReplacement",
			"ReadRegularFile",
			"ReplaceRegularFile",
		]);
	});

	it("reads bounded regular bytes without interpreting their encoding", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);

		await fs.writeFile(join(root, "bytes.bin"), new Uint8Array([0xff, 0, 1]));

		expect(
			Array.from(await Effect.runPromise(filesystem.ReadRegularFile("bytes.bin", 3))),
		).toEqual([255, 0, 1]);
		await expect(
			Effect.runPromise(filesystem.ReadRegularFile("bytes.bin", 2)),
		).rejects.toMatchObject({
			_tag: "BoundedRegularFileStoreError",
		});
	});

	it("rejects directories and symlinks as regular files", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);

		await fs.mkdir(join(root, "directory"));
		await fs.mkdir(join(root, "target"));
		await fs.symlink(join(root, "target"), join(root, "link"), "junction");

		await expect(
			Effect.runPromise(filesystem.ReadRegularFile("directory", 16)),
		).rejects.toMatchObject({
			_tag: "BoundedRegularFileStoreError",
		});
		await expect(
			Effect.runPromise(filesystem.ReadRegularFile("link", 16)),
		).rejects.toMatchObject({
			_tag: "BoundedRegularFileStoreError",
		});
	});

	it("publishes only the expected bytes and preserves the original mode", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);
		const path = join(root, "document.txt");

		await fs.writeFile(path, "old");
		if (process.platform !== "win32") await fs.chmod(path, 0o751);

		const result = await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"));

		expect(result).toEqual({ _tag: "Replaced" });
		expect(await fs.readFile(path, "utf8")).toBe("new");
		if (process.platform !== "win32") expect((await fs.stat(path)).mode & 0o777).toBe(0o751);
	});

	it("returns Changed without mutation when the initial value differs", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);

		await fs.writeFile(join(root, "document.txt"), "external");

		expect(await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"))).toEqual({
			_tag: "Changed",
		});
		expect(await fs.readFile(join(root, "document.txt"), "utf8")).toBe("external");
	});

	it("does not move a target changed before the move", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root, {
			after_stage: async () => fs.writeFile(join(root, "document.txt"), "external"),
		});

		await fs.writeFile(join(root, "document.txt"), "old");

		expect(await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"))).toEqual({
			_tag: "Changed",
		});
		expect(await fs.readFile(join(root, "document.txt"), "utf8")).toBe("external");
	});

	it("cleans a private stage when the target disappears before backup", async () => {
		const root = await make_root();
		const interrupted = await make_filesystem(root, {
			after_stage: async () => {
				await fs.rm(join(root, "document.txt"));

				throw new Error("after_stage");
			},
		});

		await fs.writeFile(join(root, "document.txt"), "old");
		await expect(
			Effect.runPromise(replace(interrupted, "document.txt", "old", "new")),
		).rejects.toMatchObject({ _tag: "BoundedRegularFileStoreError" });

		const recovered = await make_filesystem(root);

		expect(await Effect.runPromise(replace(recovered, "document.txt", "old", "new"))).toEqual({
			_tag: "Changed",
		});
		expect(
			(await fs.readdir(root)).filter((entry) => entry.startsWith(".artisan-conditional-")),
		).toEqual([]);
	});

	it("does not overwrite an external target that appears after the move", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root, {
			after_backup: async () => fs.writeFile(join(root, "document.txt"), "external"),
		});

		await fs.writeFile(join(root, "document.txt"), "old");

		expect(await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"))).toEqual({
			_tag: "Changed",
		});
		expect(await fs.readFile(join(root, "document.txt"), "utf8")).toBe("external");
	});

	it("recovers staged and moved interruptions and recognizes a published retry", async () => {
		for (const point of ["after_stage", "after_backup", "after_publication"] as const) {
			const root = await make_root();
			const interrupted = await make_filesystem(root, {
				[point]: async () => {
					throw new Error(point);
				},
			});

			await fs.writeFile(join(root, "document.txt"), "old");
			await expect(
				Effect.runPromise(replace(interrupted, "document.txt", "old", "new")),
			).rejects.toMatchObject({
				_tag: "BoundedRegularFileStoreError",
			});

			const recovered = await make_filesystem(root);
			const result = await Effect.runPromise(
				replace(recovered, "document.txt", "old", "new"),
			);

			expect(result._tag).toBe(
				point === "after_publication" ? "AlreadyReplaced" : "Replaced",
			);
			expect(await fs.readFile(join(root, "document.txt"), "utf8")).toBe("new");
		}
	});

	it("requires explicit finalization and makes exact retries idempotent", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);
		const artifacts = artifact_paths(root, "replace-operation", "document.txt");

		await fs.writeFile(join(root, "document.txt"), "old");

		expect(await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"))).toEqual({
			_tag: "Replaced",
		});
		expect(await fs.stat(artifacts.stage)).toBeTruthy();
		expect(
			(await fs.readdir(root)).some((entry) => entry.startsWith(artifacts.backup_prefix)),
		).toBe(true);

		expect(await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"))).toEqual({
			_tag: "AlreadyReplaced",
		});

		await Effect.runPromise(finalize(filesystem, "document.txt", "old", "new"));
		await Effect.runPromise(finalize(filesystem, "document.txt", "old", "new"));

		expect(await fs.readdir(root)).not.toContain(
			`.artisan-conditional-${artifact_namespace("replace-operation", "document.txt")}.stage`,
		);
		expect(
			(await fs.readdir(root)).some((entry) => entry.startsWith(artifacts.backup_prefix)),
		).toBe(false);
	});

	it.each(["after_backup_cleanup", "after_stage_cleanup"] as const)(
		"recovers finalization interrupted %s",
		async (point) => {
			const root = await make_root();
			let interrupted = true;
			const filesystem = await make_filesystem(root, {
				[point]: async () => {
					if (interrupted) {
						interrupted = false;
						throw new Error(point);
					}
				},
			});

			await fs.writeFile(join(root, "document.txt"), "old");
			await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"));

			await expect(
				Effect.runPromise(finalize(filesystem, "document.txt", "old", "new")),
			).rejects.toMatchObject({ _tag: "BoundedRegularFileStoreError" });
			await Effect.runPromise(finalize(filesystem, "document.txt", "old", "new"));
			expect(
				(await fs.readdir(root)).filter((entry) =>
					entry.startsWith(".artisan-conditional-"),
				),
			).toEqual([]);
		},
	);

	it("refuses finalization when the stage is missing but the backup remains", async () => {
		const root = await make_root();
		const artifacts = artifact_paths(root, "replace-operation", "document.txt");
		const filesystem = await make_filesystem(root);

		await fs.writeFile(join(root, "document.txt"), "old");
		await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"));
		await fs.rm(artifacts.stage);

		await expect(
			Effect.runPromise(finalize(filesystem, "document.txt", "old", "new")),
		).rejects.toMatchObject({ _tag: "BoundedRegularFileStoreError" });
		expect(
			(await fs.readdir(root)).some((entry) => entry.startsWith(artifacts.backup_prefix)),
		).toBe(true);
	});

	it.each(["absent", "external"] as const)(
		"refuses finalization while the published target is %s",
		async (state) => {
			const root = await make_root();
			const artifacts = artifact_paths(root, "replace-operation", "document.txt");
			const filesystem = await make_filesystem(
				root,
				state === "absent"
					? {
							after_backup: async () => {
								throw new Error("after_backup");
							},
						}
					: {},
			);

			await fs.writeFile(join(root, "document.txt"), "old");

			if (state === "absent") {
				await expect(
					Effect.runPromise(replace(filesystem, "document.txt", "old", "new")),
				).rejects.toMatchObject({ _tag: "BoundedRegularFileStoreError" });
			} else {
				await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"));
				await fs.rename(join(root, "document.txt"), join(root, "published.txt"));
				await fs.writeFile(join(root, "document.txt"), "external");
			}

			await expect(
				Effect.runPromise(finalize(filesystem, "document.txt", "old", "new")),
			).rejects.toMatchObject({ _tag: "BoundedRegularFileStoreError" });
			expect(await fs.stat(artifacts.stage)).toBeTruthy();
			expect(
				(await fs.readdir(root)).some((entry) => entry.startsWith(artifacts.backup_prefix)),
			).toBe(true);
		},
	);

	it("preserves 0777 through a restrictive POSIX umask", async () => {
		if (process.platform === "win32") return;

		const root = await make_root();
		const filesystem = await make_filesystem(root);
		const path = join(root, "document.txt");
		const previous_umask = process.umask(0o077);

		try {
			await fs.writeFile(path, "old", { mode: 0o777 });
			await fs.chmod(path, 0o777);
			expect(
				await Effect.runPromise(replace(filesystem, "document.txt", "old", "new")),
			).toEqual({
				_tag: "Replaced",
			});
			expect((await fs.stat(path)).mode & 0o777).toBe(0o777);
		} finally {
			process.umask(previous_umask);
		}
	});

	it("fails closed for a byte-correct stage with the wrong mode", async () => {
		if (process.platform === "win32") return;

		const root = await make_root();
		const filesystem = await make_filesystem(root);
		const artifacts = artifact_paths(root, "forged-stage", "document.txt");

		await fs.writeFile(join(root, "document.txt"), "old");
		await fs.chmod(join(root, "document.txt"), 0o751);
		await fs.writeFile(artifacts.stage, "new", { mode: 0o600 });

		await expect(
			Effect.runPromise(replace(filesystem, "document.txt", "old", "new", "forged-stage")),
		).rejects.toMatchObject({ _tag: "BoundedRegularFileStoreError" });
		expect(await fs.readFile(join(root, "document.txt"), "utf8")).toBe("old");
	});

	it("does not let a visible non-UUID backup entry poison discovery", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);
		const observer = await make_observer(root);
		const artifacts = artifact_paths(root, "junk-backup", "document.txt");
		const junk = `${artifacts.backup_prefix}junk`;

		await fs.writeFile(join(root, "document.txt"), "old");
		await fs.writeFile(join(root, junk), "visible");

		expect((await Effect.runPromise(observer.List("."))).map((entry) => entry.path)).toContain(
			junk,
		);
		expect(
			await Effect.runPromise(
				replace(filesystem, "document.txt", "old", "new", "junk-backup"),
			),
		).toEqual({
			_tag: "Replaced",
		});
	});

	it("preserves external bytes in the before-backup replacement race", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root, {
			before_backup: async () => fs.writeFile(join(root, "document.txt"), "external"),
		});

		await fs.writeFile(join(root, "document.txt"), "old");

		expect(await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"))).toEqual({
			_tag: "Changed",
		});
		expect(await fs.readFile(join(root, "document.txt"), "utf8")).toBe("external");
	});

	it.each(["exact", "different"] as const)(
		"converges concurrent %s operation IDs without raw errors",
		async (kind) => {
			const root = await make_root();
			const barrier = make_barrier(2);
			const first = await make_filesystem(root, { before_backup: barrier.enter });
			const second = await make_filesystem(root, { before_backup: barrier.enter });
			const operation_ids = kind === "exact" ? ["same", "same"] : ["first", "second"];

			await fs.writeFile(join(root, "document.txt"), "old");
			const outcomes = await Promise.allSettled(
				[first, second].map((filesystem, index) =>
					Effect.runPromise(
						replace(filesystem, "document.txt", "old", "new", operation_ids[index]),
					),
				),
			);
			const failures = outcomes.filter(
				(outcome): outcome is PromiseRejectedResult => outcome.status === "rejected",
			);

			expect(failures).toEqual([]);

			const results = outcomes.flatMap((outcome) =>
				outcome.status === "fulfilled" ? [outcome.value] : [],
			);

			if (kind === "exact") {
				expect(results.map((result) => result._tag).sort()).toEqual([
					"AlreadyReplaced",
					"Replaced",
				]);
			} else {
				expect(results.map((result) => result._tag).sort()).toEqual([
					"Changed",
					"Replaced",
				]);
			}
		},
	);

	it("hides owned artifacts and fails closed for corrupt artifacts", async () => {
		const root = await make_root();
		const operation_id = "hidden-artifact";
		const namespace = (await import("node:crypto"))
			.createHash("sha256")
			.update(operation_id)
			.update("\0")
			.update("document.txt")
			.digest("hex");
		const artifact = `.artisan-conditional-${namespace}.stage`;
		const filesystem = await make_filesystem(root);
		const observer = await make_observer(root);

		await fs.writeFile(join(root, "document.txt"), "old");
		await fs.writeFile(join(root, artifact), "malicious");

		expect(
			(await Effect.runPromise(observer.List("."))).map((entry) => entry.path),
		).not.toContain(artifact);
		await expect(
			Effect.runPromise(filesystem.ReadRegularFile(artifact, 1024)),
		).rejects.toMatchObject({
			_tag: "BoundedRegularFileStoreError",
		});
		await expect(
			Effect.runPromise(replace(filesystem, "document.txt", "old", "new", operation_id)),
		).rejects.toMatchObject({ _tag: "BoundedRegularFileStoreError" });
	});

	it("does not emit owned artifacts through Watch", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root, {
			after_stage: async () => fs.writeFile(join(root, "watch-complete.txt"), "done"),
		});
		const observer = await make_observer(root);

		await fs.writeFile(join(root, "document.txt"), "old");
		const changes = Effect.runPromise(
			observer.Watch(".").pipe(
				Stream.takeUntil((change) => change.path === "watch-complete.txt"),
				Stream.runCollect,
				Effect.timeout("3 seconds"),
			),
		);

		await new Promise((resolve) => setTimeout(resolve, 30));
		await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"));

		expect(
			(await changes)
				.map((change) => change.path)
				.some((path) => path.startsWith(".artisan-conditional-")),
		).toBe(false);
	});

	it("leaves conditional receipts after successful publication", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);

		await fs.writeFile(join(root, "document.txt"), "old");
		await Effect.runPromise(replace(filesystem, "document.txt", "old", "new"));

		expect(
			(await fs.readdir(root)).filter((entry) => entry.startsWith(".artisan-conditional-")),
		).toHaveLength(2);
	});
});
