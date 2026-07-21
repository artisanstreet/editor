import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	guidance_hash,
	make_codex_guidance_adapter,
	normalize_guidance_content,
} from "../../modules/backend/src/guidance/provider-mirrors";
import { make_test_native_guidance_adapter } from "./guidance-test-adapter";
import {
	GuidanceFileStore,
	GuidanceFileStoreLive,
} from "../../modules/backend/src/guidance/file-store";

const temporary_directories: Array<string> = [];

async function make_directory() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-guidance-files-"));

	temporary_directories.push(directory);

	return directory;
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("guidance file and provider adapters", () => {
	it("normalizes representation differences into one stable content hash", () => {
		const variants = ["Use Layers.", "Use Layers.\n", "\uFEFFUse Layers.\r\n"];

		expect(variants.map(normalize_guidance_content)).toEqual([
			"Use Layers.\n",
			"Use Layers.\n",
			"Use Layers.\n",
		]);
		expect(new Set(variants.map(guidance_hash))).toHaveLength(1);
	});

	it("writes atomically and creates collision-safe, non-destructive backups", async () => {
		const directory = await make_directory();
		const canonical = join(directory, "nested", "GLOBAL.md");
		const backups = join(directory, "backups");

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const files = yield* GuidanceFileStore;

				yield* files.WriteAtomic(canonical, "first\n");
				const backup = yield* files.CopyToBackup(canonical, backups, "before.md");
				yield* files.WriteAtomic(canonical, "second\n");
				const collision = yield* files
					.CopyToBackup(canonical, backups, "before.md")
					.pipe(Effect.exit);

				return { backup, collision, current: yield* files.Read(canonical) };
			}).pipe(Effect.provide(GuidanceFileStoreLive)),
		);

		expect(Option.getOrThrow(result.current).content).toBe("second\n");
		expect(await readFile(result.backup, "utf8")).toBe("first\n");
		expect(result.collision._tag).toBe("Failure");
	});

	it("conditionally replaces only the observed provider value and preserves races", async () => {
		const directory = await make_directory();
		const provider = join(directory, "provider", "GUIDANCE.md");
		const backups = join(directory, "backups");

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const files = yield* GuidanceFileStore;

				yield* files.WriteAtomic(provider, "Observed A\n");
				const written = yield* files.ReplaceAtomic({
					backup_name: "observed-a.md",
					backups_directory: backups,
					content: "Canonical\n",
					expected_hash: guidance_hash("Observed A\n"),
					path: provider,
				});
				yield* files.WriteAtomic(provider, "Concurrent B\n");
				const changed = yield* files.ReplaceAtomic({
					backup_name: "concurrent-b.md",
					backups_directory: backups,
					content: "Replacement\n",
					expected_hash: guidance_hash("Canonical\n"),
					path: provider,
				});

				return { changed, written };
			}).pipe(Effect.provide(GuidanceFileStoreLive)),
		);

		expect(result.written).toMatchObject({
			_tag: "Written",
			backup_path: expect.any(String),
		});
		expect(await readFile(result.written.backup_path!, "utf8")).toBe("Observed A\n");
		expect(result.changed).toMatchObject({
			_tag: "Changed",
			backup_path: expect.any(String),
		});
		expect(await readFile(result.changed.backup_path!, "utf8")).toBe("Concurrent B\n");
		expect(await readFile(provider, "utf8")).toBe("Concurrent B\n");
	});

	it("uses a nonempty Codex override and otherwise falls back to the explicit global file", async () => {
		const directory = await make_directory();
		const override = join(directory, "codex", "override.md");
		const agents = join(directory, "codex", "AGENTS.md");
		const claude_path = join(directory, "claude", "CLAUDE.md");

		await mkdir(join(directory, "codex"), { recursive: true });
		await mkdir(join(directory, "claude"), { recursive: true });
		await writeFile(agents, "Agents value\n", "utf8");
		await writeFile(override, "Override value\n", "utf8");
		await writeFile(claude_path, "Claude value\n", "utf8");

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const codex = yield* make_codex_guidance_adapter(override, agents);
				const claude = yield* make_test_native_guidance_adapter("claude", claude_path);

				return {
					claude: yield* claude.Discover,
					codex: yield* codex.Discover,
				};
			}).pipe(Effect.provide(GuidanceFileStoreLive)),
		);

		expect(result.codex).toMatchObject({
			_tag: "Present",
			content: "Override value\n",
			path: override,
		});
		expect(result.claude).toMatchObject({
			_tag: "Present",
			content: "Claude value\n",
			path: claude_path,
		});

		await writeFile(override, "", "utf8");

		const fallback = await Effect.runPromise(
			Effect.gen(function* () {
				const codex = yield* make_codex_guidance_adapter(override, agents);

				return yield* codex.Discover;
			}).pipe(Effect.provide(GuidanceFileStoreLive)),
		);

		expect(fallback).toMatchObject({
			_tag: "Present",
			content: "Agents value\n",
			path: agents,
		});
	});
});
