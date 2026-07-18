import { existsSync, readFileSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

import { initSync, parse } from "es-module-lexer";
import { describe, expect, it } from "vitest";

interface PackageBoundary {
	readonly allowed_artisan: ReadonlySet<string>;
	readonly entry: string;
	readonly forbidden_external: ReadonlySet<string>;
	readonly forbid_node: boolean;
	readonly name: string;
}

const workspace_root = resolve(import.meta.dirname, "../../..");
const frontend_contract_registry = readFileSync(
	resolve(workspace_root, "modules/frontend/src/lib/contracts/frontend-contract-registry.ts"),
	"utf8",
);

interface NormalizedSurface {
	readonly id: string;
	readonly owner: string;
	readonly state: string;
	readonly surface: string;
}

function normalized_surfaces(): ReadonlyArray<NormalizedSurface> {
	return [...frontend_contract_registry.matchAll(/\{([\s\S]*?)\n\t\},/g)].flatMap((match) => {
		const body = match[1] ?? "";
		const id = /\bid: "([^"]+)"/.exec(body)?.[1];
		const owner = /\bowner: "([^"]+)"/.exec(body)?.[1];
		const state = /\bstate: "([^"]+)"/.exec(body)?.[1];
		const surface = /\bsurface: "([^"]+)"/.exec(body)?.[1];

		return id && owner && state && surface ? [{ id, owner, state, surface }] : [];
	});
}

const boundaries: ReadonlyArray<PackageBoundary> = [
	{
		name: "protocol",
		entry: resolve(workspace_root, "modules/protocol/src/index.ts"),
		allowed_artisan: new Set(),
		forbidden_external: new Set(["drizzle-orm"]),
		forbid_node: true,
	},
	{
		name: "engines",
		entry: resolve(workspace_root, "modules/engines/src/index.ts"),
		allowed_artisan: new Set(),
		forbidden_external: new Set(["@artisan/backend", "@artisan/transport", "drizzle-orm"]),
		forbid_node: false,
	},
	{
		name: "transport client",
		entry: resolve(workspace_root, "modules/transport/src/client.ts"),
		allowed_artisan: new Set(["@artisan/protocol"]),
		forbidden_external: new Set(["@artisan/backend", "@artisan/engines", "drizzle-orm"]),
		forbid_node: true,
	},
];

function resolve_source(importer: string, specifier: string) {
	const unresolved = resolve(dirname(importer), specifier);

	return [unresolved, `${unresolved}.ts`, join(unresolved, "index.ts")].find(existsSync);
}

function external_imports(entry: string) {
	const visited = new Set<string>();
	const pending = [entry];
	const imports = new Set<string>();

	initSync();
	while (pending.length > 0) {
		const file = pending.pop()!;
		if (visited.has(file)) {
			continue;
		}

		visited.add(file);
		const [records] = parse(readFileSync(file, "utf8"), file);
		for (const record of records) {
			if (!record.n) {
				continue;
			}
			if (!record.n.startsWith(".")) {
				imports.add(record.n);
				continue;
			}

			const dependency = resolve_source(file, record.n);
			if (!dependency) {
				throw new Error(
					`Unresolved import ${record.n} from ${relative(workspace_root, file)}`,
				);
			}
			pending.push(dependency);
		}
	}

	return imports;
}

describe("deep architecture and public-surface normalization", () => {
	for (const boundary of boundaries) {
		it(`keeps the ${boundary.name} public graph inside its declared layer`, () => {
			const violations = [...external_imports(boundary.entry)].filter(
				(specifier) =>
					(boundary.forbidden_external.has(specifier) ||
						(boundary.forbid_node && specifier.startsWith("node:")) ||
						(specifier.startsWith("@artisan/") &&
							!boundary.allowed_artisan.has(specifier))) &&
					!boundary.allowed_artisan.has(specifier),
			);

			expect(violations).toEqual([]);
		});
	}

	it("keeps every renderer surface in one explicit normalized ownership state", () => {
		const normalized = normalized_surfaces();
		const ids = normalized.map(({ id }) => id);

		expect(new Set(ids).size).toBe(ids.length);
		for (const entry of normalized) {
			expect(entry.id).toMatch(/^[a-z0-9]+(?:[.-][a-z0-9]+)*$/);
			expect(
				entry.state === "live"
					? ["artisan_client", "frontend"]
					: entry.state === "fixture"
						? ["fixture"]
						: ["missing"],
			).toContain(entry.owner);
		}
	});

	it("keeps unavailable product domains explicit instead of normalizing fixtures as live", () => {
		const normalized = normalized_surfaces();
		const dependency_gates = new Map([
			["desktop-shell.electron-bootstrap", "blocked"],
			["left.marketplace", "blocked"],
			["right.permissions-usage", "blocked"],
			["right.previews", "blocked"],
			["right.processes-ports", "fixture"],
		]);

		for (const [id, state] of dependency_gates) {
			expect(normalized.find((entry) => entry.id === id)).toMatchObject({
				state,
			});
		}
	});
});
