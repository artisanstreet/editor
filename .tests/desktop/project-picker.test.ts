import { createHash, randomUUID } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, normalize, resolve } from "node:path";

import { Effect, Option } from "effect";
import { describe, expect, it } from "vitest";

import { DesktopProjectPickerError, SelectDesktopProjectDirectory } from "@artisan/desktop";

const normalize_path = (path: string) => {
	const normalized_path = normalize(resolve(path)).replaceAll("\\", "/");

	return normalized_path.replace(/(?<!^[A-Za-z]:)\/$/, "");
};

describe("desktop project picker", () => {
	it("treats native dialog cancellation as an ordinary absent project", async () => {
		const selected = await Effect.runPromise(
			SelectDesktopProjectDirectory({
				ShowOpenDialog: () => Effect.succeed({ canceled: true, filePaths: [] }),
			}),
		);

		expect(Option.isNone(selected)).toBe(true);
	});

	it("normalizes one selected directory into the backend-compatible project reference", async () => {
		const directory = await mkdtemp(join(tmpdir(), `artisan-project-picker-${randomUUID()}-`));

		try {
			const selected = await Effect.runPromise(
				SelectDesktopProjectDirectory({
					ShowOpenDialog: () =>
						Effect.succeed({ canceled: false, filePaths: [directory] }),
				}),
			);
			const root_path = normalize_path(directory);

			expect(Option.getOrThrow(selected)).toEqual({
				display_name: basename(root_path),
				project_id: `project_${createHash("sha256").update(root_path).digest("hex")}`,
				root_path,
			});
		} finally {
			await rm(directory, { force: true, recursive: true });
		}
	});

	it("fails closed when the native dialog returns anything except one directory", async () => {
		const result = await Effect.runPromise(
			SelectDesktopProjectDirectory({
				ShowOpenDialog: () => Effect.succeed({ canceled: false, filePaths: [] }),
			}),
		).then(
			() => undefined,
			(cause) => cause,
		);

		expect(result).toMatchObject({
			_tag: "DesktopProjectPickerError",
			operation: "validate",
		} as DesktopProjectPickerError);
	});
});
