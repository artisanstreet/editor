import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("thread departure attention acknowledgement loading", () => {
	it("admits departure acknowledgement after local tracking without foreground IPC", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");
		const start = rail.indexOf("const AcknowledgeDepartedThread =");
		const end = rail.indexOf("\n\t/**\n\t * Unread floats", start);
		const departure = rail.slice(start, end);

		expect(start).toBeGreaterThanOrEqual(0);
		expect(end).toBeGreaterThan(start);
		expect(departure).toContain("AdvanceThreadReadTracking(read_tracking");
		expect(departure).toContain("read_tracking = transition.state;");
		expect(departure).toContain("if (transition.acknowledgement === undefined) return;");
		expect(departure).toContain("const acknowledgement = client.Command({");
		expect(departure).toContain("Effect.forkScoped,");
		expect(departure).not.toMatch(/yield\*\s*client\.Command/u);
		expect(departure).not.toMatch(/Effect\.(?:forever|retry|sleep)/u);
		expect(departure.indexOf("read_tracking = transition.state;")).toBeLessThan(
			departure.indexOf("const acknowledgement = client.Command({"),
		);
		expect(departure).toContain('type: "thread.attention.acknowledge",');
	});
});
