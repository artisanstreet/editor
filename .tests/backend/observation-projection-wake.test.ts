import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime, Option, PubSub } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EngineObservation } from "@artisan/engines";
import { make_observation_recording } from "../../modules/backend/src/persistence/orchestration/observation-recording";
import { make_database_layer, Database } from "../../modules/backend/src/persistence/database";
import {
	ConversationItems,
	SurfaceItems,
	SurfaceUsageTotals,
	Threads,
	OrchestrationRuns,
} from "../../modules/backend/src/persistence/tables";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const Delta = (observation_id: string, sequence: number): EngineObservation => ({
	_tag: "agent_message_delta",
	artisan_run_id: "run_wake",
	delta: observation_id,
	item_id: "item_wake",
	observation_id,
	phase: "unspecified",
	raw: { engine_id: "engine_wake", frame: {}, transport: "test" },
	sequence,
	turn_id: "turn_wake",
});

const Usage = (observation_id: string, sequence: number): EngineObservation => ({
	_tag: "usage",
	artisan_run_id: "run_wake",
	basis: "cumulative",
	input_tokens: sequence,
	observation_id,
	raw: { engine_id: "engine_wake", frame: {}, transport: "test" },
	sequence,
});

const Completed = (observation_id: string, sequence: number): EngineObservation => ({
	_tag: "agent_message_completed",
	artisan_run_id: "run_wake",
	item_id: "item_wake",
	message: "Done",
	observation_id,
	phase: "final",
	raw: { engine_id: "engine_wake", frame: {}, transport: "test" },
	sequence,
	turn_id: "turn_wake",
});

const Filtered = (observation_id: string, sequence: number): EngineObservation => ({
	_tag: "terminal_activity",
	activity_id: observation_id,
	artisan_run_id: "run_wake",
	observation_id,
	output: "private terminal output",
	raw: { engine_id: "engine_wake", frame: {}, transport: "test" },
	sequence,
	state: "output",
});

const SurfaceOnly = (observation_id: string, sequence: number): EngineObservation => ({
	_tag: "native_action",
	action: "provider_event",
	artisan_run_id: "run_wake",
	detail: "Provider-native work",
	observation_id,
	raw: { engine_id: "engine_wake", frame: {}, transport: "test" },
	sequence,
});

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-observation-wake-"));
	directories.push(directory);
	return join(directory, "artisan.db");
}

function make_runtime(database_path: string) {
	let ids = 0;
	let times = 0;
	return ManagedRuntime.make(
		Layer.mergeAll(
			make_database_layer({ database_path, migrations_path }),
			Layer.succeed(
				RuntimeMetadata,
				RuntimeMetadata.of({
					instance_id: "instance_wake",
					MakeId: (prefix) => Effect.sync(() => `${prefix}_${++ids}`),
					Now: Effect.sync(() =>
						new Date(Date.parse("2026-08-15T00:00:00.000Z") + ++times).toISOString(),
					),
				}),
			),
		),
	);
}

const Seed = Effect.gen(function* () {
	const database = yield* Database;
	const now = "2026-08-15T00:00:00.000Z";
	yield* database.client.insert(Threads).values({
		created_at: now,
		thread_id: "thread_wake",
		title: "Wake",
		updated_at: now,
	});
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "agent_wake",
		created_at: now,
		engine_id: "engine_wake",
		run_id: "run_wake",
		status: "running",
		thread_id: "thread_wake",
		updated_at: now,
		working_directory: "C:/wake",
	});
});

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("observation projection wakes", () => {
	it("wakes delta conversation and usage/surface projections with zero after commit", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						yield* Seed;
						const database = yield* Database;
						const metadata = yield* RuntimeMetadata;
						const pubsub = yield* PubSub.unbounded<number>();
						const published: Array<number> = [];
						const recorder = make_observation_recording(database.client, metadata, {
							Publish: (sequence) =>
								Effect.gen(function* () {
									published.push(sequence);
									yield* PubSub.publish(pubsub, sequence);
								}),
						});
						const subscription = yield* PubSub.subscribe(pubsub);
						yield* recorder.RecordObservation(Delta("delta_wake", 1));
						yield* recorder.RecordObservation(Usage("usage_wake", 2));
						const wakes = [
							yield* PubSub.take(subscription),
							yield* PubSub.take(subscription),
						];
						const totals = yield* database.client.select().from(SurfaceUsageTotals);
						return { published, totals, wakes };
					}),
				),
			);
			expect(result.wakes).toEqual([0, 0]);
			expect(result.published).toEqual([0, 0]);
			expect(result.totals).toMatchObject([{ input_tokens: 2, run_id: "run_wake" }]);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps duplicate and filtered observations silent and coalesces a multi-observation batch", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						yield* Seed;
						const database = yield* Database;
						const metadata = yield* RuntimeMetadata;
						const pubsub = yield* PubSub.unbounded<number>();
						const recorder = make_observation_recording(database.client, metadata, {
							Publish: (sequence) =>
								PubSub.publish(pubsub, sequence).pipe(Effect.asVoid),
						});
						const subscription = yield* PubSub.subscribe(pubsub);
						yield* recorder.RecordObservations([
							Delta("delta_batch", 1),
							Usage("usage_batch", 2),
						]);
						const wake = yield* PubSub.take(subscription);
						yield* recorder.RecordObservation(Delta("delta_batch", 1));
						yield* recorder.RecordObservation(Filtered("filtered", 3));
						const extra = yield* PubSub.take(subscription).pipe(
							Effect.timeoutOption("20 millis"),
						);
						return { extra, wake };
					}),
				),
			);
			expect(result.wake).toBe(0);
			expect(Option.isNone(result.extra)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("wakes once when only a surface item changed", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						yield* Seed;
						const database = yield* Database;
						const metadata = yield* RuntimeMetadata;
						const pubsub = yield* PubSub.unbounded<number>();
						const recorder = make_observation_recording(database.client, metadata, {
							Publish: (sequence) =>
								PubSub.publish(pubsub, sequence).pipe(Effect.asVoid),
						});
						const subscription = yield* PubSub.subscribe(pubsub);
						const events = yield* recorder.RecordObservation(
							SurfaceOnly("surface_only", 1),
						);
						const wake = yield* PubSub.take(subscription);
						const extra = yield* PubSub.take(subscription).pipe(
							Effect.timeoutOption("20 millis"),
						);
						const [surfaces, conversation] = yield* Effect.all([
							database.client.select().from(SurfaceItems),
							database.client.select().from(ConversationItems),
						]);
						return { conversation, events, extra, surfaces, wake };
					}),
				),
			);
			expect(result.events).toEqual([]);
			expect(result.conversation).toEqual([]);
			expect(result.surfaces).toMatchObject([
				{ observation_id: "surface_only", run_id: "run_wake" },
			]);
			expect(result.wake).toBe(0);
			expect(Option.isNone(result.extra)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("uses the latest positive journal watermark when a batch also changed projections", async () => {
		const runtime = make_runtime(await make_database_path());
		try {
			const wake = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						yield* Seed;
						const database = yield* Database;
						const metadata = yield* RuntimeMetadata;
						const pubsub = yield* PubSub.unbounded<number>();
						const recorder = make_observation_recording(database.client, metadata, {
							Publish: (sequence) =>
								PubSub.publish(pubsub, sequence).pipe(Effect.asVoid),
						});
						const subscription = yield* PubSub.subscribe(pubsub);
						const events = yield* recorder.RecordObservations([
							Delta("delta_before_event", 1),
							Completed("completed_event", 2),
						]);
						const notified = yield* PubSub.take(subscription);
						const extra = yield* PubSub.take(subscription).pipe(
							Effect.timeoutOption("20 millis"),
						);
						return { events, extra, notified };
					}),
				),
			);
			expect(wake.events).toHaveLength(1);
			expect(wake.notified).toBe(wake.events[0]?.journal_sequence);
			expect(wake.notified).toBeGreaterThan(0);
			expect(Option.isNone(wake.extra)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("stores an engine's summary title at run settle and bumps metadata only when it changes", async () => {
		const Terminal = (
			run_id: string,
			observation_id: string,
			summary_title?: string,
		): EngineObservation => ({
			_tag: "run_terminal",
			artisan_run_id: run_id,
			observation_id,
			raw: { engine_id: "engine_wake", frame: {}, transport: "test" },
			sequence: 1,
			state: "completed",
			...(summary_title === undefined ? {} : { summary_title }),
		});
		const runtime = make_runtime(await make_database_path());
		try {
			const readings = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						yield* Seed;
						const database = yield* Database;
						const metadata = yield* RuntimeMetadata;
						const recorder = make_observation_recording(database.client, metadata, {
							Publish: () => Effect.void,
						});
						const AddRun = (run_id: string) =>
							database.client.insert(OrchestrationRuns).values({
								agent_id: "agent_wake",
								created_at: "2026-08-15T00:00:00.000Z",
								engine_id: "engine_wake",
								run_id,
								status: "running",
								thread_id: "thread_wake",
								updated_at: "2026-08-15T00:00:00.000Z",
								working_directory: "C:/wake",
							});
						const ReadThread = Effect.gen(function* () {
							const [thread] = yield* database.client
								.select({
									metadata_version: Threads.metadata_version,
									summary_title: Threads.summary_title,
								})
								.from(Threads);
							return thread;
						});

						yield* recorder.RecordObservation(
							Terminal("run_wake", "terminal_titled", "Harness session title"),
						);
						const titled = yield* ReadThread;
						yield* AddRun("run_wake_repeat");
						yield* recorder.RecordObservation(
							Terminal("run_wake_repeat", "terminal_repeat", "Harness session title"),
						);
						const repeated = yield* ReadThread;
						yield* AddRun("run_wake_refined");
						yield* recorder.RecordObservation(
							Terminal("run_wake_refined", "terminal_refined", "Refined title"),
						);
						const refined = yield* ReadThread;
						yield* AddRun("run_wake_untitled");
						yield* recorder.RecordObservation(
							Terminal("run_wake_untitled", "terminal_untitled"),
						);
						const untouched = yield* ReadThread;
						return { refined, repeated, titled, untouched };
					}),
				),
			);
			expect(readings.titled).toEqual({
				metadata_version: 1,
				summary_title: "Harness session title",
			});
			expect(readings.repeated).toEqual({
				metadata_version: 1,
				summary_title: "Harness session title",
			});
			expect(readings.refined).toEqual({
				metadata_version: 2,
				summary_title: "Refined title",
			});
			expect(readings.untouched).toEqual({
				metadata_version: 2,
				summary_title: "Refined title",
			});
		} finally {
			await runtime.dispose();
		}
	});
});
