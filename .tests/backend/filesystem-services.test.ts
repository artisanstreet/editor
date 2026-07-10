import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Filesystem } from "../../modules/backend/src/filesystem/filesystem";
import { make_node_filesystem_layer } from "../../modules/backend/src/filesystem/node-filesystem";

const roots: Array<string> = [];

async function make_root(prefix = "artisan filesystem ") {
	const root = await fs.mkdtemp(join(tmpdir(), prefix));

	roots.push(root);

	return root;
}

async function make_filesystem(root: string, watch_capacity = 256) {
	return Effect.runPromise(
		Effect.service(Filesystem).pipe(
			Effect.provide(make_node_filesystem_layer({ root, watch_capacity })),
		),
	);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("Filesystem", () => {
	it("rejects traversal and nested symlink-parent escapes while allowing safe nesting", async () => {
		const root = await make_root();
		const outside = await make_root("artisan filesystem outside ");

		await fs.writeFile(join(outside, "secret.txt"), "secret");
		await fs.symlink(outside, join(root, "linked"), "junction");
		await fs.symlink(join(outside, "missing-target"), join(root, "dangling"), "junction");
		await fs.writeFile(join(root, "source.txt"), "source");

		const filesystem = await make_filesystem(root);
		const traversal = await Effect.runPromise(
			filesystem.Resolve("../secret.txt").pipe(Effect.flip),
		);
		const read_escape = await Effect.runPromise(
			filesystem.ReadText("linked/secret.txt").pipe(Effect.flip),
		);
		const create_escape = await Effect.runPromise(
			filesystem.CreateDirectory("linked/new/deep").pipe(Effect.flip),
		);
		const write_escape = await Effect.runPromise(
			filesystem
				.WriteAtomic("linked/new.txt", new TextEncoder().encode("blocked"))
				.pipe(Effect.flip),
		);
		const dangling_escape = await Effect.runPromise(
			filesystem.CreateDirectory("dangling/new/deep").pipe(Effect.flip),
		);
		const rename_escape = await Effect.runPromise(
			filesystem.Rename("source.txt", "linked/moved.txt").pipe(Effect.flip),
		);

		await Effect.runPromise(filesystem.CreateDirectory("safe/nested/directory"));

		expect(traversal._tag).toBe("FilesystemError");
		expect(read_escape._tag).toBe("FilesystemError");
		expect(create_escape._tag).toBe("FilesystemError");
		expect(write_escape._tag).toBe("FilesystemError");
		expect(dangling_escape._tag).toBe("FilesystemError");
		expect(rename_escape._tag).toBe("FilesystemError");
		expect((await filesystem.Stat("safe/nested/directory").pipe(Effect.runPromise)).kind).toBe(
			"directory",
		);
		expect(await fs.readFile(join(root, "source.txt"), "utf8")).toBe("source");
		await expect(fs.stat(join(outside, "missing-target"))).rejects.toMatchObject({
			code: "ENOENT",
		});
	});

	it("reads binary, text, and metadata and moves deletes into managed trash", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);
		const bytes = new Uint8Array([0, 1, 2, 255]);

		await Effect.runPromise(filesystem.CreateDirectory("src/nested"));
		await Effect.runPromise(filesystem.CreateFile("src/data.bin", bytes));
		await Effect.runPromise(
			filesystem.CreateFile("src/nested/note.txt", new TextEncoder().encode("hello")),
		);

		const metadata = await Effect.runPromise(filesystem.Stat("src/data.bin"));
		const listing = await Effect.runPromise(filesystem.List("src"));
		const trash_path = await Effect.runPromise(filesystem.DeleteToTrash("src/data.bin"));

		expect(Array.from(await Effect.runPromise(filesystem.Read("src/nested/note.txt")))).toEqual(
			Array.from(new TextEncoder().encode("hello")),
		);
		expect(await Effect.runPromise(filesystem.ReadText("src/nested/note.txt"))).toBe("hello");
		expect(metadata).toMatchObject({ kind: "file", path: "src/data.bin", size: 4 });
		expect(metadata.created_at).toMatch(/^\d{4}-\d{2}-\d{2}T/);
		expect(listing.map((entry) => entry.path)).toEqual(["src/data.bin", "src/nested"]);
		expect(trash_path).toMatch(/^\.artisan-trash\//);
	});

	it("protects the project root and managed trash from rename and delete", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);

		await Effect.runPromise(filesystem.CreateFile("file.txt"));
		await Effect.runPromise(filesystem.DeleteToTrash("file.txt"));
		await fs.symlink(join(root, ".artisan-trash"), join(root, "trash-alias"), "junction");

		const delete_root = await Effect.runPromise(
			filesystem.DeleteToTrash(".").pipe(Effect.flip),
		);
		const rename_root = await Effect.runPromise(
			filesystem.Rename(".", "renamed-root").pipe(Effect.flip),
		);
		const delete_trash = await Effect.runPromise(
			filesystem.DeleteToTrash(".artisan-trash").pipe(Effect.flip),
		);
		const rename_trash = await Effect.runPromise(
			filesystem.Rename(".artisan-trash", "other-trash").pipe(Effect.flip),
		);
		const write_through_alias = await Effect.runPromise(
			filesystem
				.WriteAtomic("trash-alias/injected.txt", new TextEncoder().encode("blocked"))
				.pipe(Effect.flip),
		);

		expect(delete_root._tag).toBe("FilesystemError");
		expect(rename_root._tag).toBe("FilesystemError");
		expect(delete_trash._tag).toBe("FilesystemError");
		expect(rename_trash._tag).toBe("FilesystemError");
		expect(write_through_alias._tag).toBe("FilesystemError");
		expect((await fs.stat(root)).isDirectory()).toBe(true);
		expect((await fs.stat(join(root, ".artisan-trash"))).isDirectory()).toBe(true);
		await expect(fs.stat(join(root, ".artisan-trash", "injected.txt"))).rejects.toMatchObject({
			code: "ENOENT",
		});
	});

	it("overwrites atomically and removes temporary files after failure", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);

		await Effect.runPromise(
			filesystem.CreateFile("document.txt", new TextEncoder().encode("old")),
		);

		if (process.platform !== "win32") {
			await fs.chmod(join(root, "document.txt"), 0o751);
		}

		await Effect.runPromise(
			filesystem.WriteAtomic("document.txt", new TextEncoder().encode("new")),
		);
		await Effect.runPromise(filesystem.CreateDirectory("blocked"));

		const failure = await Effect.runPromise(
			filesystem
				.WriteAtomic("blocked", new TextEncoder().encode("cannot replace directory"))
				.pipe(Effect.flip),
		);
		const root_entries = await fs.readdir(root);

		expect(await Effect.runPromise(filesystem.ReadText("document.txt"))).toBe("new");

		if (process.platform !== "win32") {
			const mode = (await fs.stat(join(root, "document.txt"))).mode & 0o777;

			expect(mode).toBe(0o751);
		}

		expect(failure._tag).toBe("FilesystemError");
		expect(root_entries.some((entry) => entry.endsWith(".tmp"))).toBe(false);
	});

	it("normalizes create and delete watch events", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root);
		const changes = Effect.runPromise(
			filesystem.Watch(".").pipe(
				Stream.filter(
					(change) =>
						change.kind !== "overflow" &&
						change.path === "watched.txt" &&
						(change.kind === "created" || change.kind === "deleted"),
				),
				Stream.take(2),
				Stream.runCollect,
				Effect.timeout("3 seconds"),
			),
		);

		await new Promise((resolve) => setTimeout(resolve, 30));
		await fs.writeFile(join(root, "watched.txt"), "hello");
		await new Promise((resolve) => setTimeout(resolve, 30));
		await fs.rm(join(root, "watched.txt"));

		expect((await changes).map((change) => change.kind)).toEqual(["created", "deleted"]);
	});

	it("reports dropped watch events when its bounded buffer fills", async () => {
		const root = await make_root();
		const filesystem = await make_filesystem(root, 1);
		const overflow = Effect.runPromise(
			filesystem.Watch(".").pipe(
				Stream.tap(() => Effect.sleep("75 millis")),
				Stream.filter((change) => change.kind === "overflow"),
				Stream.take(1),
				Stream.runCollect,
				Effect.timeout("3 seconds"),
			),
		);

		await new Promise((resolve) => setTimeout(resolve, 30));
		await Promise.all(
			Array.from({ length: 40 }, (_, index) =>
				fs.writeFile(join(root, `event-${index}.txt`), String(index)),
			),
		);

		const [change] = await overflow;

		expect(change).toMatchObject({ kind: "overflow" });
		expect(change?.kind === "overflow" ? change.dropped : 0).toBeGreaterThan(0);
	});
});
