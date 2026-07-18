import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	ParseGitNumstat,
	ParseGitStatus,
	ParseGitWorktrees,
} from "../../modules/backend/src/git/git-parsers";

const encoder = new TextEncoder();
const oid = "1".repeat(40);
const other_oid = "2".repeat(40);

describe("Git machine-readable parsers", () => {
	it("parses attached porcelain-v2 status and preserves odd NUL-delimited paths", async () => {
		const bytes = encoder.encode(
			[
				`# branch.oid ${oid}`,
				"# branch.head main",
				"# branch.upstream origin/main",
				"# branch.ab +2 -1",
				`1 M. N... 100644 100644 100644 ${oid} ${other_oid} tab\tand\nspace name.txt`,
				`2 R. N... 100644 100644 100644 ${oid} ${other_oid} R100 renamed\nname.txt`,
				"old\tname.txt",
				"? --leading-option\nname.txt",
				"",
			].join("\0"),
		);

		const status = await Effect.runPromise(ParseGitStatus(bytes));

		expect(status.head).toEqual({ _tag: "attached", branch: "main", oid });
		expect(status.upstream).toEqual({
			_tag: "tracked",
			ahead: 2,
			behind: 1,
			ref: "origin/main",
		});
		expect(status.files).toEqual([
			expect.objectContaining({
				kind: "ordinary",
				path: "tab\tand\nspace name.txt",
				staged: true,
				status: "M.",
			}),
			expect.objectContaining({
				kind: "renamed",
				original_path: "old\tname.txt",
				path: "renamed\nname.txt",
				status: "R.",
			}),
			expect.objectContaining({
				kind: "untracked",
				path: "--leading-option\nname.txt",
				status: "??",
			}),
		]);
	});

	it("models detached and unborn HEAD without empty-string sentinels", async () => {
		const detached = await Effect.runPromise(
			ParseGitStatus(encoder.encode(`# branch.oid ${oid}\0# branch.head (detached)\0`)),
		);
		const unborn = await Effect.runPromise(
			ParseGitStatus(encoder.encode("# branch.oid (initial)\0# branch.head topic/new\0")),
		);

		expect(detached.head).toEqual({ _tag: "detached", oid });
		expect(unborn.head).toEqual({ _tag: "unborn", branch: "topic/new" });
	});

	it("fails closed for invalid UTF-8 and truncated rename records", async () => {
		const invalid_utf8 = await Effect.runPromise(
			ParseGitStatus(Uint8Array.of(0xff)).pipe(Effect.flip),
		);
		const truncated_rename = await Effect.runPromise(
			ParseGitStatus(
				encoder.encode(
					[
						`# branch.oid ${oid}`,
						"# branch.head main",
						`2 R. N... 100644 100644 100644 ${oid} ${other_oid} R100 renamed.txt`,
						"",
					].join("\0"),
				),
			).pipe(Effect.flip),
		);

		expect(invalid_utf8.format).toBe("status_v2");
		expect(truncated_rename.format).toBe("status_v2");
	});

	it("parses multiple worktrees with detached, locked, and prunable metadata", async () => {
		const worktrees = await Effect.runPromise(
			ParseGitWorktrees(
				encoder.encode(
					[
						"worktree C:/repo root",
						`HEAD ${oid}`,
						"branch refs/heads/main",
						"",
						"worktree C:/odd\nlinked",
						`HEAD ${other_oid}`,
						"detached",
						"locked user reason",
						"prunable",
						"",
						"",
					].join("\0"),
				),
			),
		);

		expect(worktrees).toEqual([
			{
				bare: false,
				branch: "refs/heads/main",
				current: false,
				detached: false,
				head: oid,
				path: "C:/repo root",
			},
			{
				bare: false,
				current: false,
				detached: true,
				head: other_oid,
				locked_reason: "user reason",
				path: "C:/odd\nlinked",
				prunable_reason: "",
			},
		]);
	});

	it("parses numeric, binary, and rename numstat records without locale text", async () => {
		const stats = await Effect.runPromise(
			ParseGitNumstat(
				encoder.encode(
					[
						"3\t1\tordinary\nname.txt",
						"-\t-\tbinary.dat",
						"1\t2\t",
						"old\tname.txt",
						"new\nname.txt",
						"",
					].join("\0"),
				),
			),
		);

		expect(stats).toEqual({ additions: 4, binary_files: 1, deletions: 3, files: 3 });
	});
});
