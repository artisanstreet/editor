import { existsSync, readFileSync } from "node:fs";
import { dirname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { initSync, parse } from "es-module-lexer";
import { describe, expect, it } from "vitest";

interface ImportGraph {
	readonly external_specifiers: ReadonlySet<string>;
	readonly files: ReadonlySet<string>;
}

const package_root = fileURLToPath(new URL("../../modules/transport", import.meta.url));

function resolve_relative_source(importer: string, specifier: string) {
	const unresolved = resolve(dirname(importer), specifier);
	const candidates = [unresolved, `${unresolved}.ts`, join(unresolved, "index.ts")];

	return candidates.find(existsSync);
}

function collect_import_graph(entry: string): ImportGraph {
	const external_specifiers = new Set<string>();
	const files = new Set<string>();
	const pending = [entry];

	initSync();

	while (pending.length > 0) {
		const file = pending.pop()!;

		if (files.has(file)) {
			continue;
		}

		files.add(file);
		const source = readFileSync(file, "utf8");
		const [imports] = parse(source, file);

		for (const imported of imports) {
			if (!imported.n) {
				continue;
			}

			if (!imported.n.startsWith(".")) {
				external_specifiers.add(imported.n);

				continue;
			}

			const resolved = resolve_relative_source(file, imported.n);

			if (!resolved) {
				throw new Error(`Could not resolve ${imported.n} from ${file}`);
			}

			pending.push(resolved);
		}
	}

	return { external_specifiers, files };
}

function expect_renderer_safe_graph(entry: string) {
	const graph = collect_import_graph(entry);
	const forbidden_external = [...graph.external_specifiers].filter(
		(specifier) => specifier === "@artisan/backend" || specifier.startsWith("node:"),
	);
	const forbidden_files = [...graph.files].filter((file) => {
		const relative = normalize(file.slice(package_root.length + 1)).replaceAll("\\", "/");

		return (
			relative === "src/server.ts" ||
			relative === "src/server-contract.ts" ||
			relative === "src/stream-source.ts" ||
			relative === "src/node-message-port.ts" ||
			relative.startsWith("src/internal/server-")
		);
	});

	expect(forbidden_external).toEqual([]);
	expect(forbidden_files).toEqual([]);
}

describe("transport package boundaries", () => {
	it("exports explicit client, connector, wire, and backend-only server entries", () => {
		const manifest = JSON.parse(readFileSync(join(package_root, "package.json"), "utf8"));

		expect(manifest.exports).toMatchObject({
			"./client": "./src/client.ts",
			"./connector": "./src/connector.ts",
			"./server": "./src/server.ts",
			"./wire": "./src/wire.ts",
		});
	});

	it("keeps the supported client and convenience entries renderer-safe", () => {
		expect_renderer_safe_graph(join(package_root, "src/client.ts"));
		expect_renderer_safe_graph(join(package_root, "src/index.ts"));
	});
});
