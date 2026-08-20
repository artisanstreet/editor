import { mkdtemp, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { ProjectDirectoryService, make_desktop_backend_runtime } from "@artisan/backend";

import { make_fake_engine } from "../engines/harness/fake-engine";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_root() {
	const root = await mkdtemp(join(tmpdir(), "artisan-project-directory-desktop-"));

	temporary_directories.push(root);

	return root;
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("desktop project directory composition", () => {
	/**
	 * The desktop path resolves this layer during its startup phase and injects
	 * it, which once bypassed the composition's dependency wiring: the Forge
	 * then died at first use with "Service not found: Artisan/ProjectLocator"
	 * and never bound a port, so the editor could not boot at all.
	 */
	it("wires the injected desktop directory service to its locator", async () => {
		const root = await make_root();

		const runtime = make_desktop_backend_runtime({
			database_path: join(root, "artisan.db"),
			engines: [make_fake_engine({ engine_id: "codex" })],
			migrations_path,
			project_directory_roots: [root],
		});

		try {
			const listing = await runtime.runPromise(
				Effect.gen(function* () {
					const directories = yield* ProjectDirectoryService;

					return yield* directories.List({});
				}),
			);

			expect(listing.directories.map((entry) => entry.kind)).toContain("root");
		} finally {
			await runtime.dispose();
		}
	});
});
