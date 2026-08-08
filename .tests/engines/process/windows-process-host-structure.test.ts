import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const host_path = "modules/engines/src/process/windows-process-host.ts";
const entry_path = "modules/engines/src/process/windows-process-host-entry.ts";

describe("Windows process host Effect boundary", () => {
	it("runs one scoped, schema-decoded host program", () => {
		const source = readFileSync(host_path, "utf8");
		const entry = readFileSync(entry_path, "utf8");

		expect(source).toContain("Schema.decodeUnknownEffect(ClaimMessage)");
		expect(source).toContain("Schema.decodeUnknownEffect(StartMessage)");
		expect(source).toContain("Effect.acquireRelease");
		expect(source).toContain("Queue.unbounded<unknown>()");
		expect(source).toContain("Deferred.make");
		expect(source).not.toContain("NodeRuntime.runMain");
		expect(entry.match(/NodeRuntime\.runMain\(/g)).toHaveLength(1);
		expect(source).not.toMatch(/Effect\.run(?:Fork|Promise|Sync)/);
		expect(source).not.toMatch(/^let\s+/m);
		expect(source.split(/\r?\n/).length).toBeLessThan(600);
	});
});
