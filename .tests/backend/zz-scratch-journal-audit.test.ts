import { copyFile, mkdtemp, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";

import { Effect, Exit, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { EventEnvelope } from "@artisan/protocol";

const source = join(process.env.LOCALAPPDATA ?? "", "Artisan", "data", "artisan.sqlite");

const temporary_directories: Array<string> = [];

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) =>
				rm(directory, { force: true, recursive: true }).catch(() => undefined),
			),
	);
});

describe("scratch: production journal decode audit", () => {
	it("decodes every persisted event with the live strict decoder", async () => {
		expect(existsSync(source)).toBe(true);
		const directory = await mkdtemp(join(tmpdir(), "artisan-journal-audit-"));
		temporary_directories.push(directory);
		const copy = join(directory, "artisan.sqlite");
		await copyFile(source, copy);
		for (const suffix of ["-wal", "-shm"]) {
			if (existsSync(source + suffix)) {
				await copyFile(source + suffix, copy + suffix);
			}
		}

		const db = new DatabaseSync(copy, { readOnly: true });
		const rows = db
			.prepare(
				`SELECT sequence AS journal_sequence, stream_id, stream_sequence AS sequence,
				        schema_version, event_id, correlation_id, causation_id, origin,
				        raw_origin_json, event_type, thread_id, run_id, agent_id,
				        payload_json, occurred_at
				 FROM journal_events ORDER BY sequence`,
			)
			.all() as Array<any>;
		db.close();

		const decode_effect = Schema.decodeUnknownEffect(EventEnvelope, {
			onExcessProperty: "error",
		});
		const decode = (input: unknown) => Effect.runSyncExit(decode_effect(input));
		const failures = new Map<
			string,
			{ count: number; first_error: string; first_seq: number }
		>();
		let ok = 0;

		for (const event of rows) {
			let payload: unknown;
			let raw_origin: unknown;
			try {
				payload = JSON.parse(event.payload_json);
				raw_origin =
					event.raw_origin_json === null ? undefined : JSON.parse(event.raw_origin_json);
			} catch (parse_error) {
				const entry = failures.get(event.event_type) ?? {
					count: 0,
					first_error: `json parse: ${parse_error}`,
					first_seq: event.journal_sequence,
				};
				entry.count += 1;
				failures.set(event.event_type, entry);
				continue;
			}

			const result = decode({
				protocol_version: 1,
				schema_version: event.schema_version,
				kind: "event",
				message_id: event.event_id,
				correlation_id: event.correlation_id,
				causation_id: event.causation_id,
				stream_id: event.stream_id,
				sequence: event.sequence,
				journal_sequence: event.journal_sequence,
				thread_id: event.thread_id,
				...(event.run_id === null ? {} : { run_id: event.run_id }),
				...(event.agent_id === null ? {} : { agent_id: event.agent_id }),
				origin: event.origin,
				...(raw_origin === undefined ? {} : { raw_origin }),
				sent_at: event.occurred_at,
				payload,
			});

			if (Exit.isFailure(result)) {
				const entry = failures.get(event.event_type) ?? {
					count: 0,
					first_error: String(result.cause).slice(0, 500),
					first_seq: event.journal_sequence,
				};
				entry.count += 1;
				failures.set(event.event_type, entry);
			} else if (event.event_type !== (result.value.payload as any).type) {
				const entry = failures.get(event.event_type) ?? {
					count: 0,
					first_error: "stored type mismatch",
					first_seq: event.journal_sequence,
				};
				entry.count += 1;
				failures.set(event.event_type, entry);
			} else {
				ok += 1;
			}
		}

		console.log(`TOTAL ${rows.length} | OK ${ok} | FAILED ${rows.length - ok}`);
		for (const [type, info] of [...failures.entries()].sort(
			(a, b) => b[1].count - a[1].count,
		)) {
			console.log(
				`FAIL type=${type} count=${info.count} first_seq=${info.first_seq}\n  ${info.first_error.replace(/\n/g, "\n  ").slice(0, 600)}`,
			);
		}
		expect(rows.length).toBeGreaterThan(0);
	}, 120_000);
});
