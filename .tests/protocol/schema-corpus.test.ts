import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { Effect, Schema } from "effect";
import * as protocol from "@artisan/protocol";

interface CorpusEntry {
	readonly id: string;
	readonly schema: string;
	readonly expect: "accept" | "reject";
	readonly json: unknown;
}

const corpusPath = join(import.meta.dirname, "fixtures/schema-corpus.json");
const corpus = JSON.parse(readFileSync(corpusPath, "utf8")) as {
	schema: string;
	entries: readonly CorpusEntry[];
};

describe("protocol schema corpus", () => {
	it("declares the current corpus format", () => {
		expect(corpus.schema).toBe("artisan.protocol.schema-corpus/1");
	});

	it("only references schemas that exist and are inventoried", () => {
		const manifest = JSON.parse(
			readFileSync(join(import.meta.dirname, "generated/schema-manifest.json"), "utf8"),
		) as {
			families: Record<string, ReadonlyArray<{ name: string }>>;
		};
		const inventoried = new Set(
			Object.values(manifest.families).flatMap((exportsList) =>
				exportsList.map((entry) => entry.name),
			),
		);
		for (const entry of corpus.entries) {
			expect(inventoried.has(entry.schema), entry.id).toBe(true);
			expect(protocol, `${entry.schema} must exist in @artisan/protocol`).toHaveProperty(
				entry.schema,
			);
		}
	});

	for (const entry of corpus.entries) {
		it(`${entry.expect} ${entry.id}`, async () => {
			const candidate = (protocol as Record<string, unknown>)[entry.schema] as {
				ast: unknown;
			};
			const decode = Schema.decodeUnknownEffect(
				candidate as Parameters<typeof Schema.decodeUnknownEffect>[0],
				{ onExcessProperty: "error" },
			);
			const attempt = Effect.runPromise(decode(entry.json));
			if (entry.expect === "accept") {
				await expect(attempt).resolves.toBeDefined();
			} else {
				await expect(attempt).rejects.toThrow();
			}
		});
	}
});
