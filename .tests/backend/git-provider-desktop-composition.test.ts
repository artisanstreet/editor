import { mkdtemp, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { GitProviderRegistry, make_desktop_backend_runtime } from "../../modules/backend/src/index";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const roots: Array<string> = [];

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => rm(root, { force: true, recursive: true })));
});

describe("desktop Git provider composition", () => {
	it("keeps the backend available while projecting a missing optional GitHub CLI", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-git-provider-desktop-"));

		roots.push(root);

		const runtime = make_desktop_backend_runtime({
			database_path: join(root, "artisan.db"),
			git_provider_platform: {
				command: "artisan-gh-command-that-does-not-exist",
			},
			migrations_path,
			model_behaviour_platform: {
				codex_command: "artisan-codex-command-that-does-not-exist",
				home_directory: root,
			},
		});

		try {
			const state = await runtime.runPromise(
				Effect.gen(function* () {
					const registry = yield* GitProviderRegistry;
					const resolution = yield* registry.ResolveHost("github.com");
					const provider = yield* registry.Get("github");
					const inspection = yield* provider.Inspect;

					return { inspection, resolution };
				}),
			);

			expect(state.resolution).toEqual({
				_tag: "resolved",
				host: "github.com",
				provider_id: "github",
			});
			expect(state.inspection.installation).toEqual({ _tag: "missing" });
			expect(state.inspection.authentication).toEqual([
				{
					accounts: [],
					active_account: { _tag: "none" },
					host: "github.com",
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});
});
