import { readFileSync, readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace_root = resolve(import.meta.dirname, "../..");
const production_sources = [
	"modules/backend/src/runtime/backend-runtime.ts",
	"modules/backend/src/guidance/provider-mirrors.ts",
	"modules/backend/src/model-behaviour/model-behaviour-provider.ts",
	"modules/forge/src",
	"modules/engines/src",
	"modules/frontend/src",
];

const forbidden_provider_tokens = ["claude", "anthropic", "CLAUDE_CONFIG_DIR", ".claude"];

const source_files = (path: string): ReadonlyArray<string> => {
	if (statSync(path).isFile()) return [path];
	const entries = readdirSync(path, { withFileTypes: true });
	return entries.flatMap((entry) => {
		const child = resolve(path, entry.name);
		if (entry.name === "fixtures") return [];
		if (entry.isDirectory()) return source_files(child);
		return entry.name.endsWith(".ts") || entry.name.endsWith(".sv") ? [child] : [];
	});
};

describe("Codex-only production boundary", () => {
	it("contains no alternate-provider config, adapter, or UI token", () => {
		for (const source of production_sources) {
			for (const path of source_files(resolve(workspace_root, source))) {
				const content = readFileSync(path, "utf8");

				for (const token of forbidden_provider_tokens) {
					expect(content.toLowerCase()).not.toContain(token.toLowerCase());
				}
			}
		}
	});
});
