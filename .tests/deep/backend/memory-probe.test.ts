import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

import { Effect, Stream } from "effect";
import { describe, expect, it } from "vitest";

import type { HelloEnvelope, OutboundControlEnvelope, SubscribeEnvelope } from "@artisan/protocol";
import { make_backend_runtime, ProtocolServer, type ProtocolConnection } from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../../modules/backend/drizzle", import.meta.url));
const source_database = process.env["ARTISAN_MEMORY_PROBE"];
const probe = source_database === undefined ? describe.skip : describe;

const now = "2026-08-18T05:00:00.000Z";

const make_hello = (): HelloEnvelope => ({
	kind: "hello",
	message_id: "memory_probe_hello",
	origin: "frontend",
	payload: {
		event_cursors: [],
		last_journal_sequence: 0,
		resume_mode: "fresh",
		supported_protocol_versions: [1],
	},
	schema_version: 1,
	sent_at: now,
});

const make_subscribe = (
	subscription_id: string,
	payload: SubscribeEnvelope["payload"],
): SubscribeEnvelope => ({
	kind: "subscribe",
	message_id: `memory_probe_${subscription_id}`,
	origin: "frontend",
	payload,
	protocol_version: 1,
	schema_version: 1,
	sent_at: now,
	subscription_id,
});

const drain_until = (
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) =>
	connection.Outbound.pipe(
		Stream.takeUntil(predicate),
		Stream.runFold(
			() => 0,
			(count) => count + 1,
		),
	);

probe("backend memory probe", () => {
	it("profiles replay and conversation-open memory against a real database", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-memory-probe-"));
		const database_path = join(directory, "artisan.sqlite");

		{
			const source = new DatabaseSync(source_database ?? "", { readOnly: true });
			source.exec(`VACUUM INTO '${database_path.replaceAll("\\", "/")}'`);
			source.close();
			const { cp } = await import("node:fs/promises");
			const { dirname, join: join_path } = await import("node:path");
			await cp(
				join_path(dirname(source_database ?? ""), "guidance"),
				join_path(directory, "guidance"),
				{ force: true, recursive: true },
			).catch(() => undefined);
		}

		let peak_rss = 0;
		const phases: Array<{ phase: string; rss_mb: number; heap_mb: number; peak_mb: number }> =
			[];
		const sampler = setInterval(() => {
			peak_rss = Math.max(peak_rss, process.memoryUsage().rss);
		}, 50);
		const mark = (phase: string) => {
			const usage = process.memoryUsage();
			peak_rss = Math.max(peak_rss, usage.rss);
			phases.push({
				phase,
				rss_mb: Math.round(usage.rss / 1048576),
				heap_mb: Math.round(usage.heapUsed / 1048576),
				peak_mb: Math.round(peak_rss / 1048576),
			});
		};

		mark("start");
		const runtime = make_backend_runtime({ database_path, migrations_path });
		try {
			const protocol_server = await runtime.runPromise(ProtocolServer);
			mark("runtime+guidance ready");

			const biggest = (() => {
				try {
					const reader = new DatabaseSync(database_path, { readOnly: true });
					try {
						return reader
							.prepare(
								"SELECT thread_id, sum(length(entity_json)) AS bytes, count(*) AS items FROM conversation_items GROUP BY thread_id ORDER BY bytes DESC LIMIT 3",
							)
							.all() as unknown as ReadonlyArray<{
							thread_id: string;
							bytes: number;
							items: number;
						}>;
					} finally {
						reader.close();
					}
				} catch {
					return [];
				}
			})();
			mark("largest threads queried");

			await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* protocol_server.Open;
						yield* connection.Receive(make_hello());
						const replayed = yield* drain_until(
							connection,
							(envelope) => envelope.kind === "replay.complete",
						);
						yield* Effect.sync(() => mark(`replay-from-origin drained (${replayed})`));

						yield* connection.Receive(
							make_subscribe("memory_probe_threads", { type: "thread.list" }),
						);
						yield* drain_until(
							connection,
							(envelope) => envelope.kind === "thread.list.snapshot",
						);
						yield* Effect.sync(() => mark("thread.list snapshot"));

						const target = biggest[0];
						if (target !== undefined) {
							yield* connection.Receive(
								make_subscribe("memory_probe_conversation", {
									thread_id: target.thread_id,
									type: "conversation",
								}),
							);
							yield* drain_until(
								connection,
								(envelope) =>
									envelope.kind === "conversation.snapshot" ||
									envelope.kind === "protocol.error",
							);
							yield* Effect.sync(() =>
								mark(
									`conversation snapshot (${target.thread_id}, ${Math.round(target.bytes / 1048576)}MB/${target.items} items)`,
								),
							);
						}
					}),
				),
			);
			mark("connection closed");
			if (globalThis.gc !== undefined) globalThis.gc();
			await new Promise((resolve) => setTimeout(resolve, 500));
			mark("settled");
		} finally {
			clearInterval(sampler);
			await writeFile(
				"C:/Users/sander/Desktop/artisan-editor/.tests/deep/backend/memory-probe-result.json",
				JSON.stringify({ phases }, null, 1),
			);
			await runtime.dispose();
		}
		expect(phases.length).toBeGreaterThan(0);
	}, 600_000);
});
