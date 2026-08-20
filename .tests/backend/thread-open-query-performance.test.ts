import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("thread-open query performance", () => {
	it("resolves canonical and public route ids in one thread lookup", () => {
		const read_model = Read("modules/backend/src/persistence/thread-read-model.ts");
		const handler = Read("modules/backend/src/protocol/rpc/query-handlers/thread.ts");
		const thread_open = handler.slice(handler.indexOf('"thread.open.query"'));

		expect(read_model).toContain("inArray(Threads.thread_id, candidate_ids)");
		expect(thread_open.match(/thread_read_model\.Lookup/g)).toHaveLength(1);
		expect(thread_open).not.toContain("exact_thread");
		expect(thread_open).toContain("conversation_read_model.ReadOpenSnapshot(thread.value)");
		expect(thread_open).toContain("onNone: () =>");
		expect(thread_open).toMatch(
			/conversation_read_model\s*\.ReadSnapshot\(\s*thread\.value\.thread_id\s*\)/u,
		);
		expect(thread_open).toContain("const [session, work, snapshot] = yield* Effect.all(");
		expect(thread_open).toContain('{ concurrency: "unbounded" }');
	});
});
