import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	claude_project_directory_name,
	claude_session_transcript_path,
	claude_session_title_from_lines,
	ReadClaudeSessionTitle,
} from "../../modules/engines/src/claude/session-title";

const title_line = (title: string) =>
	JSON.stringify({ aiTitle: title, sessionId: "s-1", type: "ai-title" });

describe("Claude session title harvesting", () => {
	it("names the project directory the way the CLI does: every non-alphanumeric becomes a dash", () => {
		expect(claude_project_directory_name("C:\\Users\\sander\\Desktop\\artisan-editor")).toBe(
			"C--Users-sander-Desktop-artisan-editor",
		);
		expect(claude_project_directory_name("/home/sander/.config/app")).toBe(
			"-home-sander--config-app",
		);
	});

	it("computes the transcript path from home, working directory, and session id", () => {
		expect(
			claude_session_transcript_path("C:\\home", "C:\\repo", "abc-123").replaceAll("\\", "/"),
		).toBe("C:/home/projects/C--repo/abc-123.jsonl");
	});

	it("takes the newest well-formed title and ignores noise around it", () => {
		expect(
			claude_session_title_from_lines([
				title_line("First title"),
				'{"type":"assistant","message":"the string \\"type\\":\\"ai-title\\" in prose"}',
				"not json at all",
				title_line("Newest title"),
				'{"type":"user"}',
			]),
		).toBe("Newest title");
		expect(claude_session_title_from_lines(['{"type":"user"}'])).toBeUndefined();
		expect(claude_session_title_from_lines([])).toBeUndefined();
	});

	it("skips a malformed newest record and falls back to the prior one", () => {
		expect(
			claude_session_title_from_lines([
				title_line("Good title"),
				'{"type":"ai-title","aiTitle":""}',
			]),
		).toBe("Good title");
	});

	it("reads the newest title from a transcript on disk and is silent about a missing one", async () => {
		const home = await mkdtemp(join(tmpdir(), "claude-title-"));
		const transcript = claude_session_transcript_path(home, "C:\\repo", "session-1");
		await mkdir(dirname(transcript), { recursive: true });
		await writeFile(
			transcript,
			`${title_line("Check entering animations")}\n{"type":"user"}\n${title_line("Final title")}\n`,
		);

		expect(
			await Effect.runPromise(
				ReadClaudeSessionTitle(transcript).pipe(Effect.provide(NodeFileSystem.layer)),
			),
		).toBe("Final title");
		expect(
			await Effect.runPromise(
				ReadClaudeSessionTitle(join(home, "projects", "C--repo", "absent.jsonl")).pipe(
					Effect.provide(NodeFileSystem.layer),
				),
			),
		).toBeUndefined();
	});
});
