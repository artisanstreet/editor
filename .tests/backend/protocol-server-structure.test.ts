import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const protocol_source = resolve("modules/backend/src/protocol");
const subscriptions_source = resolve(protocol_source, "subscriptions");
const query_handlers_source = resolve(protocol_source, "rpc", "query-handlers");
const mutation_handlers_source = resolve(protocol_source, "rpc", "mutation-handlers");
const line_count = (path: string) => readFileSync(path, "utf8").split(/\r?\n/u).length;

describe("protocol server structure", () => {
	it("keeps connection routing and subscription modules within their decomposition caps", () => {
		expect(line_count(resolve(protocol_source, "server.ts"))).toBeLessThan(1_000);

		for (const name of readdirSync(subscriptions_source).filter((entry) =>
			entry.endsWith(".ts"),
		)) {
			const source_path = resolve(subscriptions_source, name);
			expect(line_count(source_path), source_path).toBeLessThan(550);
		}

		for (const name of ["ready-dispatch.ts", "ready-mutations.ts"]) {
			const source_path = resolve(protocol_source, "rpc", name);
			expect(line_count(source_path), source_path).toBeLessThan(700);
		}

		for (const name of readdirSync(query_handlers_source).filter((entry) =>
			entry.endsWith(".ts"),
		)) {
			const source_path = resolve(query_handlers_source, name);
			expect(line_count(source_path), source_path).toBeLessThan(700);
		}

		for (const name of readdirSync(mutation_handlers_source).filter((entry) =>
			entry.endsWith(".ts"),
		)) {
			const source_path = resolve(mutation_handlers_source, name);
			expect(line_count(source_path), source_path).toBeLessThan(600);
		}
	});

	it("keeps subscription lifecycle code on Effect primitives", () => {
		for (const name of readdirSync(subscriptions_source).filter((entry) =>
			entry.endsWith(".ts"),
		)) {
			const source = readFileSync(resolve(subscriptions_source, name), "utf8");
			expect(source, name).not.toMatch(/\bnew Promise\b|\bset(?:Timeout|Interval)\b/u);
			expect(source, name).not.toContain("Effect.run");
		}
	});

	it("keeps ready dispatch typed and free of pass-through handler bundles", () => {
		const server = readFileSync(resolve(protocol_source, "server.ts"), "utf8");
		const dispatch = readFileSync(resolve(protocol_source, "rpc", "ready-dispatch.ts"), "utf8");
		const mutations = readFileSync(
			resolve(protocol_source, "rpc", "ready-mutations.ts"),
			"utf8",
		);

		expect(server).not.toContain("Effect.orDie");
		expect(dispatch).not.toContain("ReadyStaticHandlers");
		expect(mutations).not.toContain("as CommandEnvelope");
	});

	it("does not defect while constructing project protocol responses", () => {
		const project_handler = readFileSync(resolve(query_handlers_source, "project.ts"), "utf8");
		expect(project_handler).not.toContain("Effect.orDie");
	});
});
