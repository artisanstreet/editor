import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import { make_run_lifecycle } from "../../modules/backend/src/orchestration/internal/run-lifecycle";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
	Threads,
} from "../../modules/backend/src/persistence/tables";
import { ReconcileRootThreadLiveStatuses } from "../../modules/backend/src/persistence/orchestration/thread-lifecycle-status";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: string[] = [];

const MakeDatabasePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thread-status-recovery-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
};

type StrandedRun = {
	readonly group_id: string;
	readonly run_id: string;
};

type RecoveryMeasurement = {
	readonly recovered_run_ids: ReadonlyArray<string>;
	readonly select_count: number;
};

/**
 * A deliberately tiny database boundary records the SQL-shaped reads made by
 * recovery. It returns the stranded-run query first and the grouped ownership
 * query second, so the assertion measures reads rather than merely observing
 * the terminal output.
 */
const MeasureRecovery = (
	stranded: ReadonlyArray<StrandedRun>,
	groups: ReadonlyArray<{ readonly group_id: string; readonly thread_id: string }>,
): Promise<RecoveryMeasurement> => {
	let select_count = 0;
	const recovered_run_ids: Array<string> = [];
	const select = () => {
		select_count += 1;
		const result = select_count === 1 ? stranded : groups;
		return {
			from: () => ({
				where: () =>
					select_count === 1
						? { orderBy: () => Effect.succeed(result) }
						: Effect.succeed(result),
			}),
		};
	};
	const transaction = {
		select,
	};
	const lifecycle = make_run_lifecycle(
		{
			agent_name_catalog: {},
			database: {
				client: {
					transaction: (
						program: (database: typeof transaction) => Effect.Effect<unknown>,
					) => program(transaction),
				},
			},
			metadata: {},
			notifier: {},
		} as never,
		{
			publish_events: () => Effect.void,
		} as never,
		{
			transition_terminal_run: (
				_transaction: unknown,
				input: { readonly run: StrandedRun },
			) =>
				Effect.sync(() => {
					recovered_run_ids.push(input.run.run_id);
					return [];
				}),
		} as never,
	);

	return Effect.runPromise(lifecycle.recover("replacement-instance")).then(() => ({
		recovered_run_ids,
		select_count,
	}));
};

describe("agent-graph recovery query bounds", () => {
	it("uses the same two ownership reads for one stranded run or many groups", async () => {
		const single = await MeasureRecovery(
			[{ group_id: "group-a", run_id: "run-a" }],
			[{ group_id: "group-a", thread_id: "thread-a" }],
		);
		const many = await MeasureRecovery(
			[
				{ group_id: "group-a", run_id: "run-a" },
				{ group_id: "group-b", run_id: "run-b" },
				{ group_id: "group-a", run_id: "run-c" },
				{ group_id: "group-c", run_id: "run-d" },
				{ group_id: "group-missing", run_id: "run-missing" },
			],
			[
				{ group_id: "group-a", thread_id: "thread-a" },
				{ group_id: "group-b", thread_id: "thread-b" },
				{ group_id: "group-c", thread_id: "thread-c" },
			],
		);

		expect(single.select_count).toBe(2);
		expect(many.select_count).toBe(single.select_count);
		expect(many.recovered_run_ids).toEqual(["run-a", "run-b", "run-c", "run-d"]);
	});
});

describe("root thread status recovery query bounds", () => {
	it("uses one set-based update for every thread while preserving exact root status and timestamps", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakeDatabasePath(),
			migrations_path,
		});
		const initial_updated_at = "2026-08-15T10:00:00.000Z";
		const recovered_at = "2026-08-15T10:05:00.000Z";

		try {
			const threads = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.insert(Threads).values([
						{
							created_at: initial_updated_at,
							live_status: "Idle",
							thread_id: "thread_working",
							title: "Working",
							updated_at: initial_updated_at,
						},
						{
							created_at: initial_updated_at,
							live_status: "Working",
							thread_id: "thread_failed",
							title: "Failed",
							updated_at: initial_updated_at,
						},
						{
							created_at: initial_updated_at,
							live_status: "Working",
							thread_id: "thread_complete",
							title: "Complete",
							updated_at: initial_updated_at,
						},
						{
							created_at: initial_updated_at,
							live_status: "Working",
							thread_id: "thread_idle",
							title: "Idle",
							updated_at: initial_updated_at,
						},
						{
							created_at: initial_updated_at,
							live_status: "Working",
							thread_id: "thread_unchanged",
							title: "Unchanged",
							updated_at: initial_updated_at,
						},
						{
							created_at: initial_updated_at,
							live_status: "Working",
							thread_id: "thread_wrong_owner",
							title: "Wrong owner",
							updated_at: initial_updated_at,
						},
					]);
					yield* database.client.insert(OrchestrationCoordinators).values([
						{
							active_run_id: "run_working",
							agent_id: "agent_working",
							created_at: initial_updated_at,
							display_name: "Working",
							engine_id: "engine",
							role: "coordinator",
							thread_id: "thread_working",
							updated_at: initial_updated_at,
						},
						{
							active_run_id: "run_failed",
							agent_id: "agent_failed",
							created_at: initial_updated_at,
							display_name: "Failed",
							engine_id: "engine",
							role: "coordinator",
							thread_id: "thread_failed",
							updated_at: initial_updated_at,
						},
						{
							active_run_id: "run_complete",
							agent_id: "agent_complete",
							created_at: initial_updated_at,
							display_name: "Complete",
							engine_id: "engine",
							role: "coordinator",
							thread_id: "thread_complete",
							updated_at: initial_updated_at,
						},
						{
							active_run_id: "run_unchanged",
							agent_id: "agent_unchanged",
							created_at: initial_updated_at,
							display_name: "Unchanged",
							engine_id: "engine",
							role: "coordinator",
							thread_id: "thread_unchanged",
							updated_at: initial_updated_at,
						},
						{
							active_run_id: "run_foreign",
							agent_id: "agent_wrong_owner",
							created_at: initial_updated_at,
							display_name: "Wrong owner",
							engine_id: "engine",
							role: "coordinator",
							thread_id: "thread_wrong_owner",
							updated_at: initial_updated_at,
						},
					]);
					yield* database.client.insert(OrchestrationRuns).values([
						{
							agent_id: "agent_working",
							created_at: initial_updated_at,
							engine_id: "engine",
							run_id: "run_working",
							status: "running",
							thread_id: "thread_working",
							updated_at: initial_updated_at,
							working_directory: "C:/work",
						},
						{
							agent_id: "agent_failed",
							created_at: initial_updated_at,
							engine_id: "engine",
							run_id: "run_failed",
							status: "failed",
							thread_id: "thread_failed",
							updated_at: initial_updated_at,
							working_directory: "C:/work",
						},
						{
							agent_id: "agent_complete",
							created_at: initial_updated_at,
							engine_id: "engine",
							run_id: "run_complete",
							status: "completed",
							thread_id: "thread_complete",
							updated_at: initial_updated_at,
							working_directory: "C:/work",
						},
						{
							agent_id: "agent_unchanged",
							created_at: initial_updated_at,
							engine_id: "engine",
							run_id: "run_unchanged",
							status: "waiting",
							thread_id: "thread_unchanged",
							updated_at: initial_updated_at,
							working_directory: "C:/work",
						},
						{
							agent_id: "agent_foreign",
							created_at: initial_updated_at,
							engine_id: "engine",
							run_id: "run_foreign",
							status: "running",
							thread_id: "thread_foreign",
							updated_at: initial_updated_at,
							working_directory: "C:/work",
						},
					]);

					yield* database.client.transaction((transaction) =>
						ReconcileRootThreadLiveStatuses(transaction, recovered_at),
					);

					return yield* database.client
						.select({
							live_status: Threads.live_status,
							thread_id: Threads.thread_id,
							updated_at: Threads.updated_at,
						})
						.from(Threads)
						.orderBy(Threads.thread_id);
				}),
			);

			expect(threads).toEqual([
				{ live_status: "Complete", thread_id: "thread_complete", updated_at: recovered_at },
				{
					live_status: "Failed to complete",
					thread_id: "thread_failed",
					updated_at: recovered_at,
				},
				{ live_status: "Idle", thread_id: "thread_idle", updated_at: recovered_at },
				{
					live_status: "Working",
					thread_id: "thread_unchanged",
					updated_at: initial_updated_at,
				},
				{ live_status: "Working", thread_id: "thread_working", updated_at: recovered_at },
				{ live_status: "Idle", thread_id: "thread_wrong_owner", updated_at: recovered_at },
			]);
		} finally {
			await runtime.dispose();
			await Promise.all(
				temporary_directories
					.splice(0)
					.map((directory) => rm(directory, { force: true, recursive: true })),
			);
		}
	});

	it("admits exactly one database update regardless of catalog cardinality", async () => {
		let update_count = 0;
		const transaction = {
			update: () => {
				update_count += 1;
				return {
					set: () => ({ where: () => Effect.void }),
				};
			},
		};

		await Effect.runPromise(
			ReconcileRootThreadLiveStatuses(transaction as never, "2026-08-15T10:05:00.000Z"),
		);

		expect(update_count).toBe(1);
	});
});
