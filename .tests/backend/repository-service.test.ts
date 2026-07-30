import { describe, expect, it } from "vitest";

import {
	CountNulPaths,
	DefaultRemoteFor,
	IsoFromUnixSeconds,
	ParseAheadBehind,
	ParseConfiguredRemotes,
	ParseNonNegativeCount,
	WholeRecordsOf,
} from "../../modules/backend/src/git/repository-service";

const bytes_of = (value: string) => new TextEncoder().encode(value);

describe("configured remote parsing", () => {
	it("reads one remote per configured name with its host and page", () => {
		const remotes = ParseConfiguredRemotes(
			[
				"remote.origin.url git@github.com:sandersonstabo/artisan.git",
				"remote.upstream.url https://gitlab.com/group/project.git",
			].join("\n"),
		);

		expect(remotes).toEqual([
			{
				host: "github",
				name: "origin",
				url: "git@github.com:sandersonstabo/artisan.git",
				web_url: "https://github.com/sandersonstabo/artisan",
			},
			{
				host: "gitlab",
				name: "upstream",
				url: "https://gitlab.com/group/project.git",
				web_url: "https://gitlab.com/group/project",
			},
		]);
	});

	it("omits the page for a remote that has none", () => {
		const remotes = ParseConfiguredRemotes("remote.local.url /srv/mirrors/artisan.git");

		expect(remotes).toEqual([
			{ host: "unknown", name: "local", url: "/srv/mirrors/artisan.git" },
		]);
		expect(remotes[0]).not.toHaveProperty("web_url");
	});

	it("keeps a remote whose name contains dots", () => {
		const remotes = ParseConfiguredRemotes(
			"remote.my.fork.url git@github.com:someone/artisan.git",
		);

		expect(remotes.map((remote) => remote.name)).toEqual(["my.fork"]);
	});

	it("preserves a URL containing spaces", () => {
		const remotes = ParseConfiguredRemotes("remote.origin.url C:/Users/My Name/repo");

		expect(remotes[0]?.url).toBe("C:/Users/My Name/repo");
	});

	it("ignores blank lines and malformed entries", () => {
		const remotes = ParseConfiguredRemotes(
			["", "remote.origin.url https://github.com/owner/repo", "garbage", "   "].join("\n"),
		);

		expect(remotes).toHaveLength(1);
	});

	it("keeps only the first entry for a repeated remote name", () => {
		const remotes = ParseConfiguredRemotes(
			[
				"remote.origin.url https://github.com/owner/first",
				"remote.origin.url https://github.com/owner/second",
			].join("\n"),
		);

		expect(remotes).toHaveLength(1);
		expect(remotes[0]?.url).toBe("https://github.com/owner/first");
	});

	it("reads nothing from empty output", () => {
		expect(ParseConfiguredRemotes("")).toEqual([]);
	});
});

describe("default remote selection", () => {
	it("prefers origin over Git's ordering", () => {
		const remotes = ParseConfiguredRemotes(
			[
				"remote.upstream.url https://github.com/upstream/repo",
				"remote.origin.url https://github.com/owner/repo",
			].join("\n"),
		);

		expect(DefaultRemoteFor(remotes)).toBe("origin");
	});

	it("falls back to the first configured remote", () => {
		const remotes = ParseConfiguredRemotes(
			[
				"remote.upstream.url https://github.com/upstream/repo",
				"remote.fork.url https://github.com/fork/repo",
			].join("\n"),
		);

		expect(DefaultRemoteFor(remotes)).toBe("upstream");
	});

	it("names no remote for a local-only repository", () => {
		expect(DefaultRemoteFor([])).toBeUndefined();
	});
});

describe("untracked path counting", () => {
	it("counts one path per delimited record", () => {
		expect(CountNulPaths(bytes_of("src/a.ts\0src/b.ts\0"))).toBe(2);
	});

	it("counts a final record that carries no delimiter", () => {
		expect(CountNulPaths(bytes_of("src/a.ts\0src/b.ts"))).toBe(2);
	});

	it("counts a path containing spaces once", () => {
		expect(CountNulPaths(bytes_of("src/My File.ts\0"))).toBe(1);
	});

	it("counts nothing in empty output", () => {
		expect(CountNulPaths(new Uint8Array(0))).toBe(0);
	});
});

describe("bounded read truncation", () => {
	it("keeps every byte of an untruncated read", () => {
		const bytes = bytes_of("1\t2\tsrc/a.ts\0");

		expect(WholeRecordsOf({ bytes, truncated: false })).toBe(bytes);
	});

	/** A half-written record would fail the numstat parse and blank the whole reading. */
	it("drops a partial record from a truncated read", () => {
		const bytes = bytes_of("1\t2\tsrc/a.ts\u00003\t4\tsrc/part");

		expect(WholeRecordsOf({ bytes, truncated: true })).toEqual(bytes_of("1\t2\tsrc/a.ts\0"));
	});

	it("keeps nothing when a truncated read holds no complete record", () => {
		expect(WholeRecordsOf({ bytes: bytes_of("1\t2\tsrc/pa"), truncated: true })).toEqual(
			new Uint8Array(0),
		);
	});
});

describe("commit distance parsing", () => {
	/** Left counts what only the ref has, so it is how far HEAD is behind it. */
	it("reads behind from the left count and ahead from the right", () => {
		expect(ParseAheadBehind("4\t9")).toEqual({ ahead: 9, behind: 4 });
	});

	it("reads counts separated by spaces", () => {
		expect(ParseAheadBehind("0 3")).toEqual({ ahead: 3, behind: 0 });
	});

	it("reads no distance from output in another shape", () => {
		expect(ParseAheadBehind("")).toBeUndefined();
		expect(ParseAheadBehind("7")).toBeUndefined();
		expect(ParseAheadBehind("fatal: bad revision")).toBeUndefined();
	});
});

describe("count parsing", () => {
	it("reads a plain count", () => {
		expect(ParseNonNegativeCount("3")).toBe(3);
	});

	/** A repository with no stash exits non-zero and prints an error, not a number. */
	it("reads zero from output that is not a count", () => {
		expect(ParseNonNegativeCount("")).toBe(0);
		expect(ParseNonNegativeCount("-2")).toBe(0);
		expect(ParseNonNegativeCount("fatal: ambiguous argument")).toBe(0);
	});
});

describe("commit timestamp conversion", () => {
	/** Git prints the committer's own offset with %cI; the protocol carries UTC only. */
	it("converts Unix seconds into a UTC instant", () => {
		expect(IsoFromUnixSeconds("1785313812")).toBe("2026-07-29T08:30:12.000Z");
	});

	it("converts the epoch itself", () => {
		expect(IsoFromUnixSeconds("0")).toBe("1970-01-01T00:00:00.000Z");
	});

	it("converts nothing from output that is not a timestamp", () => {
		expect(IsoFromUnixSeconds("")).toBeUndefined();
		expect(IsoFromUnixSeconds("2026-07-29T10:30:12+02:00")).toBeUndefined();
		expect(IsoFromUnixSeconds("99999999999999")).toBeUndefined();
	});
});
