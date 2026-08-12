import { mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { CreateRendererDiagnosticLog } from "@artisan/desktop/renderer-diagnostics";

const temporary_directories: Array<string> = [];

afterEach(async () => {
	for (const directory of temporary_directories.splice(0)) {
		await rm(directory, { force: true, recursive: true });
	}
});

describe("renderer diagnostic persistence", () => {
	it("keeps eight sessions and retires each old session trace with its log", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-renderer-diagnostics-"));
		temporary_directories.push(directory);
		for (let index = 0; index < 9; index += 1) {
			const session = `2026-08-12T00-00-0${index}.000Z-${index}`;
			await writeFile(join(directory, `${session}.jsonl`), "{}\n");
			await writeFile(join(directory, `${session}.trace.json`), "{}");
			await writeFile(join(directory, `${session}.trace-001.json`), "{}");
		}

		const log = await Effect.runPromise(CreateRendererDiagnosticLog(directory, "latest"));
		await Effect.runPromise(log.Write({ event: "session-started" }));

		const entries = await readdir(directory);
		expect(entries.filter((entry) => entry.endsWith(".jsonl"))).toHaveLength(8);
		expect(entries).not.toContain("2026-08-12T00-00-00.000Z-0.jsonl");
		expect(entries).not.toContain("2026-08-12T00-00-00.000Z-0.trace.json");
		expect(entries).not.toContain("2026-08-12T00-00-00.000Z-0.trace-001.json");
	});
});
