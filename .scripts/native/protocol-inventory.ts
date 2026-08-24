#!/usr/bin/env node
/**
 * NATIVE-0002: protocol schema inventory and fixture manifest generator.
 *
 * Scans every module under `modules/protocol/src`, inventories the exported
 * Effect schemas, and emits a deterministic manifest recording each schema's
 * name, AST kind, owning TypeScript file, and a canonical content digest.
 *
 * The digest is sha256 over the schema's AST serialized as JSON with
 * recursively sorted object keys, so any semantic change to a schema flips
 * its digest while cosmetic reordering does not. Rust protocol packets
 * compare their ported types against this manifest before wire fixtures land
 * (MessagePack fixtures arrive with the NATIVE-0014 codec packet).
 *
 * Usage: node .scripts/native/protocol-inventory.ts
 * Output: .tests/protocol/generated/schema-manifest.json
 */

import { createHash } from "node:crypto";
import { execSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const repositoryRoot = resolve(import.meta.dirname, "../..");
const protocolSourceDirectory = join(repositoryRoot, "modules/protocol/src");
const outputPath = join(repositoryRoot, ".tests/protocol/generated/schema-manifest.json");

/** Finds exported schema declarations and their owning file via source scan. */
function scanSourceFiles() {
	const files = readdirSync(protocolSourceDirectory).filter(
		(name) => name.endsWith(".ts") && name !== "index.ts",
	);
	const declarations = [];
	for (const file of files) {
		const lines = readFileSync(join(protocolSourceDirectory, file), "utf8").split("\n");
		for (let index = 0; index < lines.length; index++) {
			const declaration = /^(?:export\s+(?:abstract\s+)?class|export\s+const)\s+(\w+)/.exec(
				lines[index],
			);
			if (!declaration) {
				continue;
			}
			// The initializer may wrap to the following line; accept either.
			const initializer = `${lines[index]}\n${lines[index + 1] ?? ""}`;
			if (!initializer.includes("Schema.")) {
				continue;
			}
			declarations.push({ name: declaration[1], file });
		}
	}
	return declarations;
}

/** Canonical JSON: object keys sorted recursively so digests are stable. */
function canonicalize(value) {
	if (Array.isArray(value)) {
		return value.map(canonicalize);
	}
	if (value !== null && typeof value === "object") {
		return Object.fromEntries(
			Object.keys(value)
				.sort()
				.map((key) => [key, canonicalize(value[key])]),
		);
	}
	if (typeof value === "bigint") {
		return value.toString();
	}
	return value;
}

function digest(value) {
	return createHash("sha256")
		.update(JSON.stringify(canonicalize(value)))
		.digest("hex");
}

function classify(ast) {
	switch (ast._tag) {
		case "Objects":
			return "object";
		case "Array":
			return "array";
		case "Union":
			return "union";
		case "Tuple":
			return "tuple";
		case "Declaration":
			return "declaration";
		default:
			return "scalar";
	}
}

const declarations = scanSourceFiles();
const protocol = await import("@artisan/protocol");

const families = new Map();
let unresolved = [];

for (const { name, file } of declarations) {
	const value = protocol[name];
	// Type-only or non-schema exports do not exist at runtime.
	if (value === undefined) {
		unresolved.push({ name, file, reason: "not-exported-at-runtime" });
		continue;
	}
	const ast = value?.ast ?? null;
	if (ast === null || typeof ast !== "object" || !ast._tag) {
		unresolved.push({ name, file, reason: "not-a-schema" });
		continue;
	}
	if (!families.has(file)) {
		families.set(file, []);
	}
	families.get(file).push({
		name,
		kind: classify(ast),
		digest: digest(ast),
	});
}

for (const exportsList of families.values()) {
	exportsList.sort((a, b) => a.name.localeCompare(b.name));
}

const manifest = {
	schema: "artisan.protocol.schema-manifest/1",
	digestAlgorithm: "sha256-canonical-json-ast",
	// Deliberately omitted: timestamps. Re-running against identical sources
	// must produce an identical, diff-clean file.
	gitRevision: (() => {
		try {
			return execSync("git rev-parse HEAD", { cwd: repositoryRoot }).toString().trim();
		} catch {
			return null;
		}
	})(),
	schemaCount: [...families.values()].reduce((sum, list) => sum + list.length, 0),
	families: Object.fromEntries([...families.entries()].sort()),
	unresolved,
};

mkdirSync(join(repositoryRoot, ".tests/protocol/generated"), { recursive: true });
writeFileSync(outputPath, `${JSON.stringify(manifest, null, "\t")}\n`);

console.log(
	`inventoried ${manifest.schemaCount} schemas across ${families.size} files -> ${outputPath}`,
);
if (unresolved.length > 0) {
	console.log(`${unresolved.length} declarations skipped (see manifest.unresolved)`);
}
