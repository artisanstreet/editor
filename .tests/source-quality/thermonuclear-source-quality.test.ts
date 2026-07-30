import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, extname, join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const modules_root = resolve("modules");
const generated_name_exceptions = [
	"modules/frontend/src/lib/assets/",
	"modules/frontend/src/lib/components/ui/",
] as const;
const generated_directories = new Set([".svelte-kit", ".vite", "build", "dist", "node_modules"]);

const ProductionSources = (directory: string): ReadonlyArray<string> =>
	readdirSync(directory).flatMap((entry) => {
		const path = join(directory, entry);
		if (statSync(path).isDirectory()) {
			return generated_directories.has(entry) ? [] : ProductionSources(path);
		}
		return [".ts", ".sv", ".svelte"].includes(extname(path)) ? [path] : [];
	});

const sources = ProductionSources(modules_root);
const WorkspacePath = (path: string) => relative(resolve("."), path).replaceAll("\\", "/");
const Source = (path: string) => readFileSync(path, "utf8");

describe("thermonuclear production source quality", () => {
	it("keeps every production TypeScript and Svelte module below 1,000 lines", () => {
		for (const path of sources) {
			expect(Source(path).split(/\r?\n/u).length, WorkspacePath(path)).toBeLessThan(1_000);
		}
	});

	it("keeps persistence and external JSON behind Effect Schema", () => {
		for (const path of sources) {
			expect(Source(path), WorkspacePath(path)).not.toMatch(/\bJSON\.parse\s*\(/u);
		}
	});

	it("keeps production code free of non-null assertions", () => {
		const assertion = /[A-Za-z0-9_\])\]]!(?:[.[\]),;:?]|$)/mu;
		for (const path of sources) {
			expect(Source(path), WorkspacePath(path)).not.toMatch(assertion);
		}
	});

	it("keeps typed failures in the Effect error channel", () => {
		for (const path of sources) {
			expect(Source(path), WorkspacePath(path)).not.toMatch(/\bEffect\.orDie\b/u);
		}
	});

	it("keeps production code free of double-cast type erasure", () => {
		for (const path of sources) {
			expect(Source(path), WorkspacePath(path)).not.toMatch(/\bas\s+unknown\s+as\b/u);
		}
	});

	it("uses contextual filenames instead of repeating the parent domain", () => {
		for (const path of sources) {
			const workspace_path = WorkspacePath(path);
			if (generated_name_exceptions.some((prefix) => workspace_path.startsWith(prefix))) {
				continue;
			}
			const parent = basename(resolve(path, ".."));
			const stem = basename(path, extname(path));
			expect(stem.startsWith(`${parent}-`), workspace_path).toBe(false);
		}
	});

	it("avoids file and directory basename collisions", () => {
		for (const path of sources) {
			const extension = extname(path);
			const same_named_directory = path.slice(0, -extension.length);
			expect(
				existsSync(same_named_directory) && statSync(same_named_directory).isDirectory(),
				WorkspacePath(path),
			).toBe(false);
		}
	});

	it("uses SER for Svelte modules that import Effect at runtime", () => {
		for (const path of sources.filter((candidate) =>
			[".sv", ".svelte"].includes(extname(candidate)),
		)) {
			const source = Source(path);
			const runtime_effect_import = [
				...source.matchAll(/import\s+([^;]+)\s+from\s+["']effect["']/gu),
			]
				.map((match) => match[1]?.trim() ?? "")
				.some((clause) => !clause.startsWith("type "));
			if (!runtime_effect_import) continue;
			expect(source, WorkspacePath(path)).toContain('<script lang="ts" effect>');
			expect(source, WorkspacePath(path)).not.toMatch(
				/\bEffect\.run(?:Fork|Promise|PromiseExit|Sync)\s*\(/u,
			);
			expect(source, WorkspacePath(path)).not.toMatch(
				/\bnew Promise\b|\bset(?:Timeout|Interval)\s*\(/u,
			);
		}
	});
});
