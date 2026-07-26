import { readFileSync, readdirSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const frontend_source = resolve("modules/frontend/src");
const source_extensions = new Set([".js", ".mjs", ".sv", ".svelte", ".ts"]);
const forbidden_specifiers = [
	"@artisan/backend",
	"@artisan/engines",
	"@artisan/transport/server",
	"@artisan/transport/node",
	"@artisan/transport/stream-source",
	"electron",
	"drizzle",
];

function source_files(directory: string): ReadonlyArray<string> {
	return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
		const path = join(directory, entry.name);

		return entry.isDirectory()
			? source_files(path)
			: source_extensions.has(extname(entry.name))
				? [path]
				: [];
	});
}

function imported_specifiers(source: string): ReadonlyArray<string> {
	const matches = source.matchAll(
		/(?:\bfrom\s*|\bimport\s*\(|\brequire\s*\(|\bimport\s*)["']([^"']+)["']/g,
	);

	return Array.from(matches, (match) => match[1]!);
}

function is_forbidden(specifier: string) {
	return (
		specifier.startsWith("node:") ||
		specifier.startsWith("drizzle-") ||
		forbidden_specifiers.some(
			(forbidden) => specifier === forbidden || specifier.startsWith(`${forbidden}/`),
		)
	);
}

describe("frontend renderer import boundary", () => {
	it("keeps backend, host, database, and Node adapters outside renderer source", () => {
		const violations = source_files(frontend_source).flatMap((file) =>
			imported_specifiers(readFileSync(file, "utf8"))
				.filter(is_forbidden)
				.map((specifier) => `${relative(frontend_source, file)} -> ${specifier}`),
		);

		expect(violations).toEqual([]);
	});

	it("allows only the renderer-safe Artisan protocol and transport client entries", () => {
		const artisan_imports = source_files(frontend_source).flatMap((file) =>
			imported_specifiers(readFileSync(file, "utf8")).filter((specifier) =>
				specifier.startsWith("@artisan/"),
			),
		);

		for (const specifier of artisan_imports) {
			expect([
				"@artisan/catalog",
				"@artisan/data/composer/placeholders.json",
				"@artisan/data/file-icons/associations.json",
				"@artisan/protocol",
				"@artisan/transport/client",
				"@artisan/transport/websocket/client",
			]).toContain(specifier);
		}
	});
});
