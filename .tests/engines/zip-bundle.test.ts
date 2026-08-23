import { createWriteStream } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { finished } from "node:stream/promises";

import { ExtractZipBundle } from "@artisan/engines";
import { afterEach, describe, expect, it } from "vitest";
import { ZipFile } from "yazl";

const roots: Array<string> = [];

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => rm(root, { force: true, recursive: true })));
});

const Root = async () => {
	const root = await mkdtemp(join(tmpdir(), "artisan-zip-bundle-"));
	roots.push(root);
	return root;
};

const WriteZip = async (
	path: string,
	entries: ReadonlyArray<{ readonly name: string; readonly value: string }>,
) => {
	const zip = new ZipFile();
	for (const entry of entries) zip.addBuffer(Buffer.from(entry.value), entry.name);
	zip.end();
	const output = createWriteStream(path, { flags: "wx" });
	zip.outputStream.pipe(output);
	await finished(output);
};

describe("managed ZIP bundles", () => {
	it("strips one declared root and extracts regular files", async () => {
		const root = await Root();
		const archive = join(root, "cursor.zip");
		const destination = join(root, "generation");
		await WriteZip(archive, [
			{ name: "dist-package/cursor-agent.cmd", value: "launcher" },
			{ name: "dist-package/node_modules/runtime/index.js", value: "runtime" },
		]);

		await ExtractZipBundle({
			archive_path: archive,
			archive_root: "dist-package",
			destination,
			maximum_output_bytes: 1_024,
		});

		await expect(readFile(join(destination, "cursor-agent.cmd"), "utf8")).resolves.toBe(
			"launcher",
		);
		await expect(
			readFile(join(destination, "node_modules", "runtime", "index.js"), "utf8"),
		).resolves.toBe("runtime");
	});

	it("rejects members outside the declared vendor root", async () => {
		const root = await Root();
		const archive = join(root, "cursor.zip");
		await WriteZip(archive, [{ name: "unexpected/file.js", value: "no" }]);

		await expect(
			ExtractZipBundle({
				archive_path: archive,
				archive_root: "dist-package",
				destination: join(root, "generation"),
				maximum_output_bytes: 1_024,
			}),
		).rejects.toThrow("outside dist-package");
	});

	it("enforces the expanded-size bound before writing a member", async () => {
		const root = await Root();
		const archive = join(root, "cursor.zip");
		await WriteZip(archive, [{ name: "dist-package/cursor-agent.cmd", value: "too-large" }]);

		await expect(
			ExtractZipBundle({
				archive_path: archive,
				archive_root: "dist-package",
				destination: join(root, "generation"),
				maximum_output_bytes: 4,
			}),
		).rejects.toThrow("expanded-size bound");
	});
});
