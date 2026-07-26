import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { GlobalGuidanceService, make_desktop_backend_runtime } from "@artisan/backend";

import { make_fake_engine } from "../engines/harness/fake-engine";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_paths() {
	const root = await mkdtemp(join(tmpdir(), "artisan-guidance-desktop-"));
	const codex_home = join(root, "codex-home");

	temporary_directories.push(root);
	await mkdir(codex_home, { recursive: true });

	return {
		canonical: join(root, "guidance", "GLOBAL.md"),
		claude: join(root, ".claude", "CLAUDE.md"),
		codex_agents: join(codex_home, "AGENTS.md"),
		codex_home,
		codex_override: join(codex_home, "AGENTS.override.md"),
		database: join(root, "artisan.db"),
		root,
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("desktop guidance composition", () => {
	it("fails closed before composing a non-Codex production engine", async () => {
		const paths = await make_paths();

		const runtime = make_desktop_backend_runtime({
			database_path: paths.database,
			engines: [make_fake_engine({ engine_id: "claude" })],
			migrations_path,
		});

		try {
			await expect(
				runtime.runPromise(
					Effect.gen(function* () {
						yield* GlobalGuidanceService;
					}),
				),
			).rejects.toMatchObject({
				_tag: "DesktopEngineConfigurationError",
				message: "Desktop production accepts only the Codex engine.",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("derives the native Codex registry from the installed Engine", async () => {
		const paths = await make_paths();

		await writeFile(paths.codex_agents, "Desktop provider guidance\n", "utf8");
		await mkdir(join(paths.root, ".claude"), { recursive: true });
		await writeFile(paths.claude, "Must remain untouched\n", "utf8");

		const runtime = make_desktop_backend_runtime({
			database_path: paths.database,
			engines: [make_fake_engine({ engine_id: "codex" })],
			guidance: { canonical_path: paths.canonical },
			guidance_platform: {
				codex_home: paths.codex_home,
				home_directory: paths.root,
			},
			migrations_path,
		});

		try {
			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					return yield* guidance.Get;
				}),
			);

			expect(snapshot.content).toBe("Desktop provider guidance\n");
			expect(snapshot.metadata.providers).toMatchObject([
				{ provider: "codex", status: "synced" },
			]);
			expect(await readFile(paths.canonical, "utf8")).toBe("Desktop provider guidance\n");
			expect(await readFile(paths.claude, "utf8")).toBe("Must remain untouched\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("honors Codex override precedence through the desktop composition", async () => {
		const paths = await make_paths();

		await writeFile(paths.codex_agents, "Fallback guidance\n", "utf8");
		await writeFile(paths.codex_override, "Override guidance\n", "utf8");

		const runtime = make_desktop_backend_runtime({
			database_path: paths.database,
			engines: [make_fake_engine({ engine_id: "codex" })],
			guidance: { canonical_path: paths.canonical },
			guidance_platform: {
				codex_home: paths.codex_home,
				home_directory: paths.root,
			},
			migrations_path,
		});

		try {
			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					return yield* guidance.Get;
				}),
			);

			expect(snapshot.content).toBe("Override guidance\n");
			expect(snapshot.metadata.providers).toMatchObject([
				{
					path: paths.codex_override,
					provider: "codex",
					status: "synced",
				},
			]);
			expect(await readFile(paths.codex_agents, "utf8")).toBe("Fallback guidance\n");
			expect(await readFile(paths.codex_override, "utf8")).toBe("Override guidance\n");
		} finally {
			await runtime.dispose();
		}
	});
});
