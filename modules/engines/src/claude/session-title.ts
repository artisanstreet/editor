import { join } from "node:path";

import { Effect, FileSystem, Option, Schema } from "effect";

/**
 * The CLI's generated session title, harvested from the session transcript.
 *
 * Claude Code names every session with a short model-written title and appends
 * it to the transcript as an `ai-title` record — within the first turn, then
 * again as the conversation evolves. The CLI itself resolves the current name
 * by taking the newest record, so this reader does the same. No stream-json
 * event carries the title; the transcript file is the only public surface.
 */

/** One transcript line carrying the CLI's generated session title. */
const ClaudeSessionTitleRecord = Schema.Struct({
	aiTitle: Schema.NonEmptyString,
	type: Schema.Literal("ai-title"),
});

const decode_title_record = Schema.decodeUnknownOption(
	Schema.fromJsonString(ClaudeSessionTitleRecord),
	{ onExcessProperty: "preserve" },
);

/**
 * A transcript past this size is not read at all. Settles are seconds apart at
 * their fastest, so the read is rare, but an unbounded `readFile` of a runaway
 * transcript would trade a nicety for memory pressure.
 */
const maximum_transcript_bytes = 64 * 1024 * 1024;

/**
 * The directory Claude Code files a working directory's transcripts under:
 * every character outside `[A-Za-z0-9]` becomes a dash, drive colon and path
 * separators included.
 */
export const claude_project_directory_name = (working_directory: string): string =>
	working_directory.replace(/[^A-Za-z0-9]/gu, "-");

/** The session transcript's path inside one Claude config home. */
export const claude_session_transcript_path = (
	home: string,
	working_directory: string,
	session_id: string,
): string =>
	join(home, "projects", claude_project_directory_name(working_directory), `${session_id}.jsonl`);

/** Newest generated title in one transcript's lines, or none. */
export const claude_session_title_from_lines = (
	lines: ReadonlyArray<string>,
): string | undefined => {
	for (let index = lines.length - 1; index >= 0; index -= 1) {
		const line = lines[index];

		if (line === undefined || !line.includes('"type":"ai-title"')) continue;

		const decoded = decode_title_record(line);

		if (Option.isSome(decoded)) return decoded.value.aiTitle;
	}

	return undefined;
};

/**
 * Reads the newest generated title from one session transcript.
 *
 * Deliberately total: a missing transcript, an unreadable file, or a malformed
 * record all mean "no title yet" — a run must never fail, or even complain,
 * because a nicety could not be read.
 */
export const ReadClaudeSessionTitle = (
	transcript_path: string,
): Effect.Effect<string | undefined, never, FileSystem.FileSystem> =>
	Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const info = yield* file_system.stat(transcript_path);

		if (info.size > maximum_transcript_bytes) return undefined;

		return claude_session_title_from_lines(
			(yield* file_system.readFileString(transcript_path, "utf8")).split("\n"),
		);
	}).pipe(Effect.catch(() => Effect.succeed(undefined)));
