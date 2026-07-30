import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const guidance_root = join(process.cwd(), "modules", "backend", "src", "guidance");

describe("guidance source structure", () => {
	it("uses contextual filenames and keeps production modules below the hard cap", async () => {
		const files = (await readdir(guidance_root)).filter((file) => file.endsWith(".ts"));

		expect(files.some((file) => file.startsWith("guidance-"))).toBe(false);

		for (const file of files) {
			const source = await readFile(join(guidance_root, file), "utf8");
			expect(source.split(/\r?\n/u).length, file).toBeLessThan(1_000);
		}
	});

	it("keeps orchestration and provider synchronization narrowly bounded", async () => {
		const service = await readFile(join(guidance_root, "service.ts"), "utf8");
		const provider_sync = await readFile(join(guidance_root, "provider-sync.ts"), "utf8");

		expect(service.split(/\r?\n/u).length).toBeLessThan(700);
		expect(provider_sync.split(/\r?\n/u).length).toBeLessThan(500);
	});
});
