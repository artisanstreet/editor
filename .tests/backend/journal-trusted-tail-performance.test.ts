import { readFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Exit } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import { make_backend_runtime } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];
const historic_rows = 1_024;

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-journal-trusted-tail-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const create_command = {
	kind: "command",
	message_id: "trusted_tail_create",
	origin: "frontend",
	payload: { title: "Trusted tail", type: "thread.create" },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T00:00:00.000Z",
	thread_id: "thread_trusted_tail",
} satisfies CommandEnvelope;

describe("JournalStore trusted live tail", () => {
	it("resumes from a validated cursor by decoding only the delta after a large corrupt history", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					const baseline = yield* journal.ReadWatermark();
					yield* database.client.run(
						`INSERT INTO event_streams (stream_id, last_sequence) VALUES ('history', ${historic_rows})`,
					);
					yield* database.client.run(`
						WITH RECURSIVE history(n) AS (
							VALUES(1)
							UNION ALL
							SELECT n + 1 FROM history WHERE n < ${historic_rows}
						)
						INSERT INTO journal_events (
							stream_id, stream_sequence, schema_version, event_id, correlation_id,
							causation_id, origin, event_type, thread_id, payload_json, occurred_at
						)
						SELECT
							'history', n, 1, 'historic-' || n, 'historic-correlation-' || n,
							'historic-causation-' || n, 'backend', 'thread.created', 'thread_history',
							'{malformed history', '2026-08-15T00:00:00.000Z'
						FROM history
					`);
					const fresh_baseline = yield* journal.ReadBaseline();
					const after_journal_sequence = baseline + historic_rows;
					const empty_resume = yield* journal.ReadResume({
						after_journal_sequence,
						stream_cursors: fresh_baseline.event_cursors,
					});
					const accepted = yield* journal.AcceptThreadCreate(create_command);
					const resume = yield* journal.ReadResume({
						after_journal_sequence,
						stream_cursors: fresh_baseline.event_cursors,
					});
					const replay = yield* journal
						.ReadReplay({ after_journal_sequence: 0 })
						.pipe(Effect.exit);
					const accepted_baseline = yield* journal.ReadBaseline();

					return {
						accepted,
						accepted_baseline,
						after_journal_sequence,
						empty_resume,
						fresh_baseline,
						replay,
						resume,
					};
				}),
			);

			expect(result.accepted.journal_sequence).toBe(result.after_journal_sequence + 1);
			expect(result.empty_resume).toEqual([]);
			expect(result.resume).toHaveLength(1);
			expect(result.resume[0]).toMatchObject({
				journal_sequence: result.after_journal_sequence + 1,
				thread_id: "thread_trusted_tail",
			});
			expect(result.fresh_baseline.event_cursors).toEqual(
				expect.arrayContaining([{ sequence: historic_rows, stream_id: "history" }]),
			);
			expect(result.fresh_baseline.journal_sequence).toBe(result.after_journal_sequence);
			expect(result.accepted_baseline.event_cursors).toEqual(
				expect.arrayContaining([
					{ sequence: historic_rows, stream_id: "history" },
					{ sequence: 1, stream_id: "thread:thread_trusted_tail" },
				]),
			);
			expect(result.accepted_baseline.journal_sequence).toBe(
				result.after_journal_sequence + 1,
			);
			expect(Exit.isFailure(result.replay)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects malformed or discontinuous resume deltas and invalid supplied cursors", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					const baseline = yield* journal.ReadBaseline();
					yield* journal.AcceptThreadCreate({
						...create_command,
						message_id: "resume_delta_create",
						thread_id: "thread_resume_delta",
					});
					const mismatched = yield* journal
						.ReadResume({
							after_journal_sequence: baseline.journal_sequence,
							stream_cursors: baseline.event_cursors.map((cursor) => ({
								...cursor,
								sequence: cursor.sequence + 1,
							})),
						})
						.pipe(Effect.exit);
					const duplicate = yield* journal
						.ReadResume({
							after_journal_sequence: baseline.journal_sequence,
							stream_cursors: [...baseline.event_cursors, ...baseline.event_cursors],
						})
						.pipe(Effect.exit);
					yield* database.client.run(
						"INSERT INTO event_streams (stream_id, last_sequence) VALUES ('gap', 2)",
					);
					yield* database.client.run(`
						INSERT INTO journal_events (
							stream_id, stream_sequence, schema_version, event_id, correlation_id,
							causation_id, origin, event_type, thread_id, payload_json, occurred_at
						) VALUES (
							'gap', 2, 1, 'gap-delta', 'gap-correlation', 'gap-causation',
							'backend', 'thread.created', 'thread_gap',
							'{"type":"thread.created","title":"Gap"}', '2026-08-15T00:00:00.000Z'
						)
					`);
					const gapped = yield* journal
						.ReadResume({
							after_journal_sequence: baseline.journal_sequence,
							stream_cursors: baseline.event_cursors,
						})
						.pipe(Effect.exit);

					return { duplicate, gapped, mismatched };
				}),
			);

			expect(Exit.isFailure(result.mismatched)).toBe(true);
			expect(Exit.isFailure(result.duplicate)).toBe(true);
			expect(Exit.isFailure(result.gapped)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects a malformed event in the resume delta", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const malformed = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					const baseline = yield* journal.ReadBaseline();
					yield* database.client.run(
						"INSERT INTO event_streams (stream_id, last_sequence) VALUES ('malformed', 1)",
					);
					yield* database.client.run(`
						INSERT INTO journal_events (
							stream_id, stream_sequence, schema_version, event_id, correlation_id,
							causation_id, origin, event_type, thread_id, payload_json, occurred_at
						) VALUES (
							'malformed', 1, 1, 'malformed-delta', 'malformed-correlation',
							'malformed-causation', 'backend', 'thread.created', 'thread_malformed',
							'{malformed delta', '2026-08-15T00:00:00.000Z'
						)
					`);
					return yield* journal
						.ReadResume({
							after_journal_sequence: baseline.journal_sequence,
							stream_cursors: baseline.event_cursors,
						})
						.pipe(Effect.exit);
				}),
			);

			expect(Exit.isFailure(malformed)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("reserves full replay validation for first trust and routes reconnect through bounded resume", () => {
		const journal_source = readFileSync(
			resolve("modules/backend/src/persistence/journal-store.ts"),
			"utf8",
		);
		const server_source = readFileSync(
			resolve("modules/backend/src/protocol/server.ts"),
			"utf8",
		);
		const mutations_source = readFileSync(
			resolve("modules/backend/src/protocol/rpc/ready-mutations.ts"),
			"utf8",
		);
		const affinity_source = readFileSync(
			resolve("modules/backend/src/threads/thread-project-affinity-coordinator.ts"),
			"utf8",
		);
		const tail = journal_source.slice(
			journal_source.indexOf("const ReadTrustedTail"),
			journal_source.indexOf("const AcceptThreadCreate"),
		);
		const resume = journal_source.slice(
			journal_source.indexOf("const ReadResume"),
			journal_source.indexOf("const ReadTrustedTail"),
		);
		const committed_tail = server_source.slice(
			server_source.indexOf("const DeliverCommittedTail"),
			server_source.indexOf("const ready_connection_runtime"),
		);
		const live_delivery = server_source.slice(
			server_source.indexOf("const DeliverJournalTail"),
			server_source.indexOf("const JournalTail"),
		);
		const baseline = journal_source.slice(
			journal_source.indexOf("const ReadBaselineInTransaction"),
			journal_source.indexOf("export const JournalStoreLive"),
		);

		expect(baseline).toContain("from(EventStreams)");
		expect(baseline).toContain("select({ journal_sequence: JournalEvents.sequence })");
		expect(baseline).not.toContain("JournalEvents.stream_sequence");
		expect(baseline).not.toContain("DeriveStreamCursors");
		expect(baseline).not.toContain("AssertMatchingCursors");
		expect(tail).toContain("where(gt(JournalEvents.sequence, after_sequence))");
		expect(tail).toContain("orderBy(asc(JournalEvents.sequence))");
		expect(tail).not.toContain("EventStreams");
		expect(tail).not.toContain("lte(JournalEvents.sequence");
		expect(resume).toContain("ReadBaselineInTransaction(transaction)");
		expect(resume).toContain("where(eq(JournalEvents.sequence, after_journal_sequence))");
		expect(resume).toContain("where(gt(JournalEvents.sequence, after_journal_sequence))");
		expect(resume).toContain("ContinueStreamCursors(");
		expect(resume).toContain("AssertMatchingCursors(");
		expect(resume).not.toContain("lte(JournalEvents.sequence");
		/**
		 * Every live-delivery path — notifier wakes and mutation commits alike —
		 * reads the trusted tail from the connection cursor through one shared
		 * operation. A mutation delivering only its own events would advance the
		 * cursor past concurrently journaled events no later wake re-reads.
		 */
		expect(committed_tail).toContain("ReadTrustedTail(current.delivered_journal_sequence)");
		expect(live_delivery).toContain("yield* DeliverCommittedTail()");
		expect(server_source).toContain(
			"ReadResume({\n\t\t\t\t\t\t\t\t\t\tafter_journal_sequence: hello.payload.last_journal_sequence,",
		);
		expect(mutations_source).toContain("runtime.DeliverCommittedTail(");
		expect(mutations_source).not.toContain("ReadTrustedTail(");
		expect(affinity_source).toContain(
			"? yield* journal.ReadReplay({ after_journal_sequence: 0 })",
		);
		expect(affinity_source).toContain(": yield* journal.ReadTrustedTail(cursor.value)");
	});
});
