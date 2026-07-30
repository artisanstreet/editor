import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const protocol_source = resolve("modules/protocol/src");
const control_source = resolve(protocol_source, "control-contract");

const line_count = (path: string) => readFileSync(path, "utf8").split(/\r?\n/u).length;

describe("control schema structure", () => {
	it("keeps the facade and every cohesive control module below 1,000 lines", () => {
		const source_paths = [
			resolve(protocol_source, "control.ts"),
			...readdirSync(control_source)
				.filter((name) => name.endsWith(".ts"))
				.map((name) => resolve(control_source, name)),
		];

		for (const source_path of source_paths) {
			expect(line_count(source_path), source_path).toBeLessThan(1_000);
		}
	});

	it("keeps the top-level control module as an explicit composition facade", () => {
		const facade = readFileSync(resolve(protocol_source, "control.ts"), "utf8");

		expect(facade).not.toContain("Schema.");
		expect(facade).toContain('export * from "./control-contract/commands";');
		expect(facade).toContain('export * from "./control-contract/lifecycle";');
		expect(facade).toContain('export * from "./control-contract/marketplace";');
		expect(facade).toContain('export * from "./control-contract/subscriptions";');
		expect(facade).toContain('export * from "./control-contract/wire";');
	});

	it("does not collide source-file and directory basenames", () => {
		const entries = readdirSync(protocol_source, { withFileTypes: true });
		const source_basenames = new Set(
			entries
				.filter((entry) => entry.isFile() && entry.name.endsWith(".ts"))
				.map((entry) => entry.name.slice(0, -3)),
		);

		for (const directory of entries.filter((entry) => entry.isDirectory())) {
			expect(source_basenames.has(directory.name), directory.name).toBe(false);
		}
	});
});
