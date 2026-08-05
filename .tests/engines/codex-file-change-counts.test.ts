import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { CountFileChangeLines, normalise_codex_notification } from "@artisan/engines";

/** The exact payload shape Codex sends for a created file: content, not a patch. */
const created_file_content =
	"# For the Next Agent\n\nYou arrive where another thought ended,\nat a desk with the lamplight still warm.\n";

const unified_diff = [
	"diff --git a/notes.md b/notes.md",
	"index 0000000..c79acf5 100644",
	"--- a/notes.md",
	"+++ b/notes.md",
	"@@ -1,2 +1,3 @@",
	" context",
	"-removed",
	"+added one",
	"+added two",
].join("\n");

const normalise = (payload: unknown) =>
	normalise_codex_notification({
		artisan_run_id: "run-1",
		frame_sequence: 1,
		method: "item/completed",
		payload,
		protocol_version: "v1",
		raw_frame_base64: "e30=",
		transport: "stdio-jsonl",
	});

const file_change_frame = (kind: unknown, diff: string) => ({
	item: {
		changes: [{ diff, kind, path: "C:\\Users\\sander\\Desktop\\artisan-test\\notes.md" }],
		id: "exec-1",
		status: "completed",
		type: "fileChange",
	},
	threadId: "thread-1",
	turnId: "turn-1",
});

describe("file change line counting", () => {
	/**
	 * The regression: a created file's payload carries no `+` markers, so reading
	 * it as a unified diff counted nothing and the card claimed +0 -0 for a file
	 * that plainly had lines.
	 */
	it("counts a created file's content as added lines, not as an empty diff", () => {
		expect(CountFileChangeLines("created", created_file_content)).toEqual({
			lines_added: 5,
			lines_deleted: 0,
		});
	});

	it("still counts a real unified diff from its markers", () => {
		expect(CountFileChangeLines("modified", unified_diff)).toEqual({
			lines_added: 2,
			lines_deleted: 1,
		});
	});

	it("counts a deleted file's content as removed lines", () => {
		expect(CountFileChangeLines("deleted", created_file_content)).toEqual({
			lines_added: 0,
			lines_deleted: 5,
		});
	});

	/**
	 * Content for a modification says what the file now holds and nothing about
	 * what it replaced. Uncounted renders as no numbers; a zero would claim the
	 * edit changed nothing.
	 */
	it("leaves a modification uncounted when the payload is content rather than a patch", () => {
		expect(CountFileChangeLines("modified", created_file_content)).toBeUndefined();
	});

	it("is not fooled by content whose lines begin with diff markers", () => {
		expect(CountFileChangeLines("created", "- milk\n- eggs\n+ extra\n")).toEqual({
			lines_added: 4,
			lines_deleted: 0,
		});
	});
});

describe("Codex file change observations", () => {
	it("reports real counts for a created file instead of zeroes", async () => {
		const observations = await Effect.runPromise(
			normalise(file_change_frame({ type: "add" }, created_file_content)),
		);

		expect(observations).toEqual([
			expect.objectContaining({
				_tag: "file",
				action: "created",
				lines_added: 5,
				lines_deleted: 0,
			}),
		]);
	});

	it("omits counts entirely for a modification reported as content", async () => {
		const [observation] = await Effect.runPromise(
			normalise(file_change_frame({ move_path: null, type: "update" }, created_file_content)),
		);

		expect(observation).toMatchObject({ _tag: "file", action: "modified" });
		expect(observation).not.toHaveProperty("lines_added");
		expect(observation).not.toHaveProperty("lines_deleted");
	});

	it("counts a modification that does arrive as a patch", async () => {
		const observations = await Effect.runPromise(
			normalise(file_change_frame({ move_path: null, type: "update" }, unified_diff)),
		);

		expect(observations).toEqual([
			expect.objectContaining({
				_tag: "file",
				action: "modified",
				lines_added: 2,
				lines_deleted: 1,
			}),
		]);
	});
});

describe("codex protocol drift", () => {
	const notify = (method: string, payload: unknown) =>
		normalise_codex_notification({
			artisan_run_id: "run-1",
			frame_sequence: 1,
			method,
			payload,
			protocol_version: "v1",
			raw_frame_base64: "e30=",
			transport: "stdio-jsonl",
		});

	/**
	 * An unmodelled item type failed the whole envelope, so every start and
	 * completion frame carrying one was reported as a malformed payload.
	 */
	it.each(["reasoning", "userMessage"])("decodes the %s item envelope", async (type) => {
		const events = await Effect.runPromise(
			notify("item/completed", {
				item: { id: "item-1", type },
				threadId: "thread-1",
				turnId: "turn-1",
			}),
		);

		expect(events).toHaveLength(1);
		expect(events[0]).toMatchObject({ _tag: "native_action" });
		expect(JSON.stringify(events)).not.toContain("Malformed");
	});

	it.each([
		"account/rateLimits/updated",
		"mcpServer/startupStatus/updated",
		"remoteControl/status/changed",
	])("recognises the %s notification", async (method) => {
		const events = await Effect.runPromise(notify(method, {}));

		expect(JSON.stringify(events)).not.toContain("Unknown Codex method");
	});

	/**
	 * Drift is permanent — providers add frames faster than adapters model them.
	 * What must not happen is a frame we cannot read becoming a transcript row
	 * that reads as a failure.
	 */
	it("marks a frame it still cannot read as a diagnostic", async () => {
		const events = await Effect.runPromise(notify("some/brand/new/method", {}));

		expect(events).toEqual([
			expect.objectContaining({ _tag: "native_action", diagnostic: true }),
		]);
	});

	it("keeps diagnostics out of the transcript entirely", () => {
		const activity = readFileSync(
			resolve("modules/backend/src/conversation/projection/activity.ts"),
			"utf8",
		);

		expect(activity).toContain(
			'observation._tag === "native_action" && observation.diagnostic',
		);
		expect(activity).toContain("return Effect.void;");
	});
});
