import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const orchestration_root = join(
	process.cwd(),
	"modules",
	"backend",
	"src",
	"persistence",
	"orchestration",
);

const line_count = (file_name: string) =>
	readFileSync(join(orchestration_root, file_name), "utf8").split(/\r?\n/u).length;

describe("orchestration persistence structure", () => {
	it("keeps the acceptance boundary narrow and every production module below 1,000 lines", () => {
		expect(line_count("acceptance.ts")).toBeLessThan(800);

		for (const file_name of [
			"acceptance.ts",
			"command-dispatch.ts",
			"contracts.ts",
			"message-attachments.ts",
			"message-command.ts",
			"outbox.ts",
			"repository.ts",
			"storage-codec.ts",
		]) {
			expect(line_count(file_name), file_name).toBeLessThan(1_000);
		}
	});

	it("keeps command dispatch transaction-local", () => {
		const acceptance = readFileSync(join(orchestration_root, "acceptance.ts"), "utf8");
		const dispatch = readFileSync(join(orchestration_root, "command-dispatch.ts"), "utf8");

		expect(acceptance).toContain("database.client.transaction");
		expect(acceptance).toContain("Effect.provideService(CommandTransaction");
		expect(dispatch).toContain("yield* CommandTransaction");
		expect(dispatch).not.toContain("database.client.transaction");
	});
});
