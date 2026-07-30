import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const capability_source = resolve("modules/backend/src/marketplace/capabilities");
const Read = (name: string) => readFileSync(resolve(capability_source, name), "utf8");
const LineCount = (name: string) => Read(name).split(/\r?\n/u).length;

describe("marketplace capability structure", () => {
	it("keeps contextual module names and every production file below 1,000 lines", () => {
		const source_names = readdirSync(capability_source).filter((name) => name.endsWith(".ts"));

		expect(source_names.filter((name) => name.startsWith("capability-"))).toEqual([]);
		for (const source_name of source_names)
			expect(LineCount(source_name), source_name).toBeLessThan(1_000);
	});

	it("keeps the repository as a narrow service composer", () => {
		const repository = Read("repository.ts");

		expect(LineCount("repository.ts")).toBeLessThan(1_000);
		expect(repository).toContain("MakeDriftPersistence");
		expect(repository).toContain("MakeInvocationPersistence");
		expect(repository).toContain("MakeLifecyclePersistence");
		expect(repository).not.toContain("const RecordDriftResolution =");
	});

	it("preserves typed capability failures instead of converting them to defects", () => {
		expect(Read("repository.ts")).not.toContain("Effect.orDie");
		expect(Read("provider-mirrors.ts")).not.toContain("Effect.orDie");
	});
});
