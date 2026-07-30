import { spawn } from "node:child_process";
import {
	appendFile,
	lstat,
	mkdir,
	mkdtemp,
	readdir,
	rm,
	symlink,
	writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Option, type Scope } from "effect";
import { describe, expect, it } from "vitest";

import {
	ClaudeCompactionCapture,
	type ClaudeCompactionLease,
	make_claude_compaction_capture_layer,
} from "@artisan/engines";

const helper_entry = fileURLToPath(
	new URL("../../modules/engines/src/claude/claude-post-compact-hook.ts", import.meta.url),
);
const now = "2026-07-30T10:00:00.000Z";

const claim = (lease: ClaudeCompactionLease) => lease.environment.ARTISAN_CLAUDE_HOOK_CLAIM!;
const mailbox = (lease: ClaudeCompactionLease) => lease.environment.ARTISAN_CLAUDE_HOOK_MAILBOX!;

const mailbox_record = async (input: {
	readonly lease: ClaudeCompactionLease;
	readonly summary: string;
	readonly transcript_path: string;
	readonly size_before_append?: number;
}) => {
	const identity = await lstat(input.transcript_path);
	return JSON.stringify({
		artisan_run_id: "capture-run",
		claim: claim(input.lease),
		hook: {
			compact_summary: input.summary,
			cwd: process.cwd(),
			hook_event_name: "PostCompact",
			session_id: "session",
			transcript_path: input.transcript_path,
			trigger: "auto",
		},
		received_at: now,
		schema_version: 1,
		transcript_identity: {
			device: String(identity.dev),
			inode: String(identity.ino),
			size_before_append: input.size_before_append ?? identity.size,
		},
	});
};

const transcript_tail = (boundary_id: string, summary: string) =>
	`${JSON.stringify({
		compactMetadata: { trigger: "auto" },
		subtype: "compact_boundary",
		type: "system",
		uuid: boundary_id,
	})}\n${JSON.stringify({
		isCompactSummary: true,
		message: { content: summary },
		type: "user",
	})}\n`;

const run_capture = (
	root: string,
	body: (lease: ClaudeCompactionLease) => Effect.Effect<void, unknown, Scope.Scope>,
	settle_timeout_ms = 150,
) =>
	Effect.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const service = yield* ClaudeCompactionCapture;
				const lease = yield* service.Prepare({
					artisan_run_id: "capture-run",
					environment: process.env,
					native_session_id: "session",
					working_directory: process.cwd(),
				});
				yield* body(lease);
				return yield* lease.Finalize;
			}).pipe(
				Effect.provide(
					make_claude_compaction_capture_layer({
						claude_config_dir: root,
						settle_timeout_ms,
					}),
				),
			),
		),
	);

const run_helper = (
	input: string,
	environment: {
		readonly mailbox: string;
		readonly transcript_path: string;
	},
) =>
	new Promise<string>((resolve, reject) => {
		const child = spawn(process.execPath, [helper_entry], {
			cwd: process.cwd(),
			env: {
				...process.env,
				ARTISAN_CLAUDE_HOOK_CLAIM: "A".repeat(43),
				ARTISAN_CLAUDE_HOOK_MAILBOX: environment.mailbox,
				ARTISAN_CLAUDE_HOOK_RUN_ID: "helper-run",
			},
			stdio: ["pipe", "pipe", "pipe"],
		});
		const stdout: Array<Buffer> = [];
		const stderr: Array<Buffer> = [];
		child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
		child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
		const timeout = setTimeout(() => {
			child.kill();
			reject(new Error("Claude PostCompact helper did not exit"));
		}, 10_000);
		child.once("error", (cause) => {
			clearTimeout(timeout);
			reject(cause);
		});
		child.once("close", (code) => {
			clearTimeout(timeout);
			if (code === 0) resolve(Buffer.concat(stdout).toString("utf8"));
			else
				reject(
					new Error(
						`Claude PostCompact helper exited ${String(code)}: ${Buffer.concat(stderr).toString("utf8")}`,
					),
				);
		});
		child.stdin.end(input);
	});

const helper_input = (summary: string, transcript_path: string) =>
	JSON.stringify({
		compact_summary: summary,
		cwd: process.cwd(),
		hook_event_name: "PostCompact",
		session_id: "session",
		transcript_path,
		trigger: "auto",
	});

describe("Claude compaction capture adversarial boundaries", () => {
	it("polls an initially empty mailbox and tolerates a partial JSON replacement", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-capture-"));
		try {
			const result = await run_capture(root, (lease) =>
				Effect.gen(function* () {
					const project = join(root, "projects", "project");
					const transcript = join(project, "session.jsonl");
					yield* Effect.tryPromise(() => mkdir(project, { recursive: true }));
					yield* Effect.tryPromise(() => writeFile(transcript, "{}\n"));
					const summary = "private summary";
					const record = yield* Effect.promise(() =>
						mailbox_record({ lease, summary, transcript_path: transcript }),
					);
					yield* Effect.forkScoped(
						Effect.sleep("25 millis").pipe(
							Effect.andThen(
								Effect.tryPromise(() =>
									writeFile(
										join(mailbox(lease), "capture.json"),
										record.slice(0, 20),
									),
								),
							),
						),
					);
					yield* Effect.forkScoped(
						Effect.sleep("50 millis").pipe(
							Effect.andThen(
								Effect.tryPromise(() =>
									writeFile(join(mailbox(lease), "capture.json"), record),
								),
							),
						),
					);
					yield* Effect.forkScoped(
						Effect.sleep("40 millis").pipe(
							Effect.andThen(
								Effect.tryPromise(() =>
									appendFile(transcript, transcript_tail("boundary-1", summary)),
								),
							),
						),
					);
				}),
			);
			expect(Option.getOrUndefined(result)).toMatchObject({
				boundary_id: "boundary-1",
				summary: "private summary",
			});
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("rejects a regular transcript outside the real Claude projects root", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-capture-"));
		try {
			const result = await run_capture(
				root,
				(lease) =>
					Effect.gen(function* () {
						yield* Effect.tryPromise(() =>
							mkdir(join(root, "projects"), { recursive: true }),
						);
						const transcript = join(root, "outside", "session.jsonl");
						yield* Effect.tryPromise(() =>
							mkdir(dirname(transcript), { recursive: true }),
						);
						yield* Effect.tryPromise(() => writeFile(transcript, "{}\n"));
						const record = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								summary: "outside",
								transcript_path: transcript,
							}),
						);
						yield* Effect.tryPromise(() =>
							appendFile(transcript, transcript_tail("outside-boundary", "outside")),
						);
						yield* Effect.tryPromise(() =>
							writeFile(join(mailbox(lease), "outside.json"), record),
						);
					}),
				50,
			);
			expect(Option.isNone(result)).toBe(true);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("rejects symlink transcripts and mailbox records when the platform permits symlinks", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-capture-"));
		try {
			const project = join(root, "projects", "project");
			await mkdir(project, { recursive: true });
			const target = join(project, "actual.jsonl");
			const transcript = join(project, "session.jsonl");
			await writeFile(target, `{}\n${transcript_tail("symlink-boundary", "symlink")}`);
			try {
				await symlink(target, transcript, "file");
			} catch (cause) {
				expect(["EACCES", "EPERM"]).toContain((cause as NodeJS.ErrnoException).code);
				return;
			}

			const transcript_result = await run_capture(
				root,
				(lease) =>
					Effect.gen(function* () {
						const record = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								size_before_append: 3,
								summary: "symlink",
								transcript_path: transcript,
							}),
						);
						yield* Effect.tryPromise(() =>
							writeFile(join(mailbox(lease), "transcript.json"), record),
						);
					}),
				50,
			);
			expect(Option.isNone(transcript_result)).toBe(true);

			const mailbox_result = await run_capture(
				root,
				(lease) =>
					Effect.gen(function* () {
						const regular = join(mailbox(lease), "regular-record");
						yield* Effect.tryPromise(() => writeFile(regular, "{}"));
						yield* Effect.tryPromise(() =>
							symlink(regular, join(mailbox(lease), "linked.json"), "file"),
						);
					}),
				50,
			);
			expect(Option.isNone(mailbox_result)).toBe(true);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("rejects a transcript replaced after the helper captured its identity", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-capture-"));
		try {
			const result = await run_capture(
				root,
				(lease) =>
					Effect.gen(function* () {
						const project = join(root, "projects", "project");
						const transcript = join(project, "session.jsonl");
						yield* Effect.tryPromise(() => mkdir(project, { recursive: true }));
						yield* Effect.tryPromise(() => writeFile(transcript, "{}\n"));
						const record = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								summary: "replaced",
								transcript_path: transcript,
							}),
						);
						yield* Effect.tryPromise(() => rm(transcript));
						yield* Effect.tryPromise(() =>
							writeFile(
								transcript,
								`{}\n${transcript_tail("replacement-boundary", "replaced")}`,
							),
						);
						yield* Effect.tryPromise(() =>
							writeFile(join(mailbox(lease), "replacement.json"), record),
						);
					}),
				50,
			);
			expect(Option.isNone(result)).toBe(true);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("fails closed when two valid records claim one boundary with different summaries", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-capture-"));
		try {
			const result = await run_capture(
				root,
				(lease) =>
					Effect.gen(function* () {
						const project = join(root, "projects", "project");
						const transcript = join(project, "session.jsonl");
						yield* Effect.tryPromise(() => mkdir(project, { recursive: true }));
						yield* Effect.tryPromise(() => writeFile(transcript, "{}\n"));
						const first_offset = (yield* Effect.tryPromise(() => lstat(transcript)))
							.size;
						yield* Effect.tryPromise(() =>
							appendFile(transcript, transcript_tail("same-boundary", "summary one")),
						);
						const second_offset = (yield* Effect.tryPromise(() => lstat(transcript)))
							.size;
						yield* Effect.tryPromise(() =>
							appendFile(transcript, transcript_tail("same-boundary", "summary two")),
						);
						const first = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								size_before_append: first_offset,
								summary: "summary one",
								transcript_path: transcript,
							}),
						);
						const second = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								size_before_append: second_offset,
								summary: "summary two",
								transcript_path: transcript,
							}),
						);
						yield* Effect.tryPromise(() =>
							writeFile(join(mailbox(lease), "a.json"), first),
						);
						yield* Effect.forkScoped(
							Effect.sleep("50 millis").pipe(
								Effect.andThen(
									Effect.tryPromise(() =>
										writeFile(join(mailbox(lease), "b.json"), second),
									),
								),
							),
						);
					}),
				150,
			);
			expect(Option.isNone(result)).toBe(true);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("requires the compact summary immediately after its boundary", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-capture-"));
		try {
			const result = await run_capture(
				root,
				(lease) =>
					Effect.gen(function* () {
						const project = join(root, "projects", "project");
						const transcript = join(project, "session.jsonl");
						yield* Effect.tryPromise(() => mkdir(project, { recursive: true }));
						yield* Effect.tryPromise(() => writeFile(transcript, "{}\n"));
						const offset = (yield* Effect.tryPromise(() => lstat(transcript))).size;
						yield* Effect.tryPromise(() =>
							appendFile(
								transcript,
								`${JSON.stringify({
									compactMetadata: { trigger: "auto" },
									subtype: "compact_boundary",
									type: "system",
									uuid: "intervening-boundary",
								})}\n${JSON.stringify({
									message: { content: "unrelated" },
									type: "assistant",
								})}\n${JSON.stringify({
									isCompactSummary: true,
									message: { content: "private summary" },
									type: "user",
								})}\n`,
							),
						);
						const record = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								size_before_append: offset,
								summary: "private summary",
								transcript_path: transcript,
							}),
						);
						yield* Effect.tryPromise(() =>
							writeFile(join(mailbox(lease), "intervening.json"), record),
						);
					}),
				50,
			);
			expect(Option.isNone(result)).toBe(true);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("selects the latest valid compaction by transcript offset", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-capture-"));
		try {
			const result = await run_capture(
				root,
				(lease) =>
					Effect.gen(function* () {
						const project = join(root, "projects", "project");
						const transcript = join(project, "session.jsonl");
						yield* Effect.tryPromise(() => mkdir(project, { recursive: true }));
						yield* Effect.tryPromise(() => writeFile(transcript, "{}\n"));
						const earlier_offset = (yield* Effect.tryPromise(() => lstat(transcript)))
							.size;
						yield* Effect.tryPromise(() =>
							appendFile(transcript, transcript_tail("earlier", "earlier summary")),
						);
						const later_offset = (yield* Effect.tryPromise(() => lstat(transcript)))
							.size;
						yield* Effect.tryPromise(() =>
							appendFile(transcript, transcript_tail("later", "later summary")),
						);
						const earlier = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								size_before_append: earlier_offset,
								summary: "earlier summary",
								transcript_path: transcript,
							}),
						);
						const later = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								size_before_append: later_offset,
								summary: "later summary",
								transcript_path: transcript,
							}),
						);
						yield* Effect.tryPromise(() =>
							Promise.all([
								writeFile(join(mailbox(lease), "a.json"), earlier),
								writeFile(join(mailbox(lease), "z.json"), later),
							]),
						);
					}),
				50,
			);
			expect(Option.getOrThrow(result)).toMatchObject({
				boundary_id: "later",
				summary: "later summary",
			});
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("fails closed instead of accepting a partial mailbox scan", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-capture-"));
		try {
			const result = await run_capture(
				root,
				(lease) =>
					Effect.gen(function* () {
						const project = join(root, "projects", "project");
						const transcript = join(project, "session.jsonl");
						yield* Effect.tryPromise(() => mkdir(project, { recursive: true }));
						yield* Effect.tryPromise(() => writeFile(transcript, "{}\n"));
						const offset = (yield* Effect.tryPromise(() => lstat(transcript))).size;
						yield* Effect.tryPromise(() =>
							appendFile(transcript, transcript_tail("bounded", "bounded summary")),
						);
						const valid = yield* Effect.promise(() =>
							mailbox_record({
								lease,
								size_before_append: offset,
								summary: "bounded summary",
								transcript_path: transcript,
							}),
						);
						yield* Effect.tryPromise(() =>
							Promise.all([
								writeFile(join(mailbox(lease), "00-valid.json"), valid),
								...Array.from({ length: 16 }, (_, index) =>
									writeFile(
										join(
											mailbox(lease),
											`${String(index + 1).padStart(2, "0")}-extra.json`,
										),
										"{}",
									),
								),
							]),
						);
					}),
				50,
			);
			expect(Option.isNone(result)).toBe(true);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("silently rejects malformed, oversized, and byte-oversized helper input and cleans temp files", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-claude-helper-"));
		try {
			const mailbox_path = join(root, "mailbox");
			const transcript = join(root, "session.jsonl");
			await mkdir(mailbox_path);
			await writeFile(transcript, "{}\n");
			const environment = { mailbox: mailbox_path, transcript_path: transcript };

			await expect(run_helper("{", environment)).resolves.toBe("");
			await expect(run_helper("x".repeat(320 * 1024 + 1), environment)).resolves.toBe("");
			await expect(
				run_helper(helper_input("é".repeat(100_000), transcript), environment),
			).resolves.toBe("");
			expect(await readdir(mailbox_path)).toEqual([]);

			await expect(
				run_helper(helper_input("valid private summary", transcript), environment),
			).resolves.toBe("");
			const files = await readdir(mailbox_path);
			expect(files).toHaveLength(1);
			expect(files[0]).toMatch(/^capture-[0-9a-f-]+\.json$/);
			expect(files.some((name) => name.endsWith(".tmp"))).toBe(false);
			const captured = JSON.parse(
				await import("node:fs/promises").then(({ readFile }) =>
					readFile(join(mailbox_path, files[0]!), "utf8"),
				),
			);
			expect(captured).toMatchObject({
				artisan_run_id: "helper-run",
				hook: { compact_summary: "valid private summary" },
			});
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});
});
