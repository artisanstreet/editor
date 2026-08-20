import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime, Option, PubSub } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import { TerminalRepository, TerminalRepositoryLive } from "@artisan/backend";
import { make_database_layer, Database } from "../../modules/backend/src/persistence/database";
import { JournalNotifier } from "../../modules/backend/src/persistence/journal-notifier";
import { Threads } from "../../modules/backend/src/persistence/tables";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const Open = (terminal_id: string): CommandEnvelope => ({
	kind: "command",
	message_id: `open:${terminal_id}`,
	origin: "frontend",
	payload: {
		args: ["--interactive"],
		cols: 100,
		executable: "fake-shell",
		rows: 30,
		terminal_id,
		type: "terminal.open",
		working_directory: "C:\\workspace",
		workspace_id: "workspace_recovery",
	},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T00:00:00.000Z",
	thread_id: "thread_recovery",
});

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-terminal-recovery-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
}

function make_runtime(database_path: string, published: Array<number>) {
	let next_id = 0;
	let next_time = 0;
	const notifier = Layer.effect(
		JournalNotifier,
		Effect.gen(function* () {
			const pubsub = yield* PubSub.unbounded<number>();
			return JournalNotifier.of({
				Publish: (journal_sequence) =>
					Effect.gen(function* () {
						published.push(journal_sequence);
						yield* PubSub.publish(pubsub, journal_sequence);
					}),
				Subscribe: PubSub.subscribe(pubsub),
			});
		}),
	);
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		Layer.succeed(
			RuntimeMetadata,
			RuntimeMetadata.of({
				instance_id: "instance_recovered",
				MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
				Now: Effect.sync(() =>
					new Date(Date.parse("2026-08-15T00:00:00.000Z") + ++next_time).toISOString(),
				),
			}),
		),
		notifier,
	);
	return ManagedRuntime.make(TerminalRepositoryLive.pipe(Layer.provideMerge(infrastructure)));
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("terminal stale recovery performance", () => {
	it("recovers stale generations atomically and publishes only the final recovery watermark", async () => {
		const published: Array<number> = [];
		const runtime = make_runtime(await make_database_path(), published);

		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const database = yield* Database;
						const repository = yield* TerminalRepository;
						const notifier = yield* JournalNotifier;
						yield* database.client.insert(Threads).values({
							created_at: "2026-08-15T00:00:00.000Z",
							thread_id: "thread_recovery",
							title: "Recovery",
							updated_at: "2026-08-15T00:00:00.000Z",
						});

						const opening = yield* repository.Claim(
							Open("terminal_alpha"),
							"instance_old",
						);
						const active = yield* repository.Claim(
							Open("terminal_bravo"),
							"instance_old",
						);
						yield* repository.CommitCommand(
							Open("terminal_bravo"),
							active.generation,
							"opened",
							{ _tag: "active", pid: 42 },
						);
						const later_opening = yield* repository.Claim(
							Open("terminal_charlie"),
							"instance_old",
						);
						const current_owner = yield* repository.Claim(
							Open("terminal_current"),
							"instance_recovered",
						);
						expect(opening.generation).toBe(1);
						expect(later_opening.generation).toBe(1);
						expect(current_owner.generation).toBe(1);

						published.splice(0);
						const subscription = yield* notifier.Subscribe;
						const recovered = yield* repository.RecoverStale(
							"instance_recovered",
							"previous backend exited",
						);
						const wake = yield* PubSub.take(subscription);
						const recovered_again = yield* repository.RecoverStale(
							"instance_recovered",
							"previous backend exited",
						);
						const extra_wake = yield* PubSub.take(subscription).pipe(
							Effect.timeoutOption("20 millis"),
						);
						const events = yield* database.client.all<{
							readonly payload_json: string;
						}>("SELECT payload_json FROM journal_events ORDER BY sequence ASC");
						const recovered_events = events
							.map(
								(event) =>
									JSON.parse(event.payload_json) as {
										action?: string;
										terminal?: { terminal_id: string };
									},
							)
							.filter((payload) => payload.action === "recovered");

						return {
							extra_wake,
							published: [...published],
							recovered,
							recovered_again,
							recovered_events,
							wake,
						};
					}),
				),
			);

			expect(result.recovered).toBe(3);
			expect(result.recovered_again).toBe(0);
			expect(result.recovered_events.map((event) => event.terminal?.terminal_id)).toEqual([
				"terminal_alpha",
				"terminal_bravo",
				"terminal_charlie",
			]);
			expect(result.published).toEqual([result.wake]);
			expect(Option.isNone(result.extra_wake)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("delegates service recovery to the repository batch instead of a stale-read commit loop", async () => {
		const source = await readFile(
			fileURLToPath(
				new URL("../../modules/backend/src/terminal/sessions.ts", import.meta.url),
			),
			"utf8",
		);

		expect(source).toContain("repository.RecoverStale(");
		expect(source).not.toMatch(/ReadStale[\\s\\S]*CommitRecovery/u);
	});

	it("admits a receipted restart from the terminal row without scanning command history", async () => {
		const source = await readFile(
			fileURLToPath(
				new URL("../../modules/backend/src/terminal/repository.ts", import.meta.url),
			),
			"utf8",
		);
		const restart_branch = source.slice(
			source.indexOf('payload.type === "terminal.restart"'),
			source.indexOf('payload.type === "terminal.pin"'),
		);

		expect(restart_branch).toContain(
			"current.stop_requested_generation !== current.generation",
		);
		expect(restart_branch).not.toContain("TerminalCommands");
	});
});
