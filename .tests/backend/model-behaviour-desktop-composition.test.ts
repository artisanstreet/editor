import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_desktop_backend_runtime,
	ModelBehaviourService,
} from "../../modules/backend/src/index";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const roots: Array<string> = [];

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("desktop Model Behaviour composition", () => {
	it("fails closed and preserves Codex config when the CLI cannot be probed", async () => {
		const root = await fs.mkdtemp(`${tmpdir()}/artisan model behaviour desktop `);
		const codex_home = join(root, "codex home");
		const config_path = join(codex_home, "config.toml");
		const original = '# retained\napi_key = "untouched"\n';

		roots.push(root);
		await fs.mkdir(codex_home, { recursive: true });
		await fs.writeFile(config_path, original, "utf8");

		const runtime = make_desktop_backend_runtime({
			database_path: join(root, "artisan.db"),
			migrations_path,
			model_behaviour_platform: {
				codex_command: "artisan-codex-command-that-does-not-exist",
				codex_home,
			},
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;
					const snapshot = yield* service.Get;
					const update = yield* service
						.Update({
							message_id: "unavailable_update",
							origin: "frontend",
							sent_at: "2026-07-11T17:00:00.000Z",
							setting_id: "auto_compaction_trigger_tokens",
							value: { type: "integer", value: 250_000 },
						})
						.pipe(Effect.exit);

					return { snapshot, update };
				}),
			);
			const codex = result.snapshot.providers.find(
				({ provider_id }) => provider_id === "codex",
			)!;

			expect(codex.status).toBe("version_unavailable");
			expect(result.update._tag).toBe("Failure");
			expect(JSON.stringify(result.update)).toContain("no_supported_provider");
			expect(await fs.readFile(config_path, "utf8")).toBe(original);
			await expect(fs.access(join(root, "model-behaviour", "backups"))).rejects.toBeDefined();
		} finally {
			await runtime.dispose();
		}
	});
});
