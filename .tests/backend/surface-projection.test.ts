import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	AgentRuns,
	OrchestrationRuns,
	SurfaceItems,
	SurfaceUsageTotals,
} from "../../modules/backend/src/persistence/tables";
import { SurfaceService } from "../../modules/backend/src/surfaces/service";
import { PersistSurfaceProjection } from "../../modules/backend/src/surfaces/surface-projection";
import { SurfaceFromEngineObservation } from "../../modules/backend/src/surfaces/engine-observation";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];
const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-surface-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};
afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("surface projection read model", () => {
	it.each([
		["malformed summary JSON", { summary_json: "{" }],
		["malformed raw-origin JSON", { raw_origin_json: "{" }],
		["schema-invalid kind", { kind: "not_a_surface_kind" }],
	])("returns a typed invariant failure for %s", async (_name, corrupt) => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const outcome = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const surfaces = yield* SurfaceService;
					yield* database.client.insert(SurfaceItems).values({
						assignment_id: null,
						category: "work",
						group_id: null,
						kind: "message",
						observation_id: "observation_1",
						occurred_at: "2026-07-18T00:00:01.000Z",
						raw_origin_json: '{"provider":"codex","reference":"observation_1"}',
						run_id: "run_1",
						sequence: 1,
						summary_json: '{"label":"Message"}',
						surface_id: "surface_1",
						thread_id: "thread_1",
						...corrupt,
					});

					return yield* Effect.exit(surfaces.List({ thread_id: "thread_1" }));
				}),
			);

			expect(outcome._tag).toBe("Failure");
			expect(JSON.stringify(outcome)).toContain("SurfaceInvariantFailed");
		} finally {
			await runtime.dispose();
		}
	});

	it("uses fixed safe mapper labels for hostile provider tool fields", () => {
		const item = SurfaceFromEngineObservation(
			{
				_tag: "tool",
				artisan_run_id: "run_1",
				observation_id: "observation_1",
				sequence: 1,
				raw: { engine_id: "codex", frame: { secret: "do-not-project" }, transport: "test" },
				tool_id: "tool_1",
				tool_name: "x".repeat(10_000),
				action: "started",
			} as any,
			{ thread_id: "thread_1", run_id: "run_1" },
			"2026-07-18T00:00:00.000Z",
		);
		expect(item).toMatchObject({
			category: "capability",
			kind: "tool",
			summary: { label: "Tool", status: "started" },
		});
		expect(JSON.stringify(item)).not.toContain("do-not-project");
	});
	it("replaces cumulative usage, adds deltas, and ignores duplicate observations", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		const usage = (
			observation_id: string,
			basis: "cumulative" | "delta",
			input_tokens?: number,
			output_tokens?: number,
		) =>
			({
				_tag: "usage",
				artisan_run_id: "run_1",
				observation_id,
				sequence: Number(observation_id.at(-1)),
				raw: { engine_id: "codex", frame: {}, transport: "test" },
				basis,
				...(input_tokens === undefined ? {} : { input_tokens }),
				...(output_tokens === undefined ? {} : { output_tokens }),
			}) as any;
		try {
			const total = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* PersistSurfaceProjection(
								transaction,
								usage("usage_1", "cumulative", 10, 5),
								{
									thread_id: "thread_1",
									run_id: "run_1",
									group_id: "group_1",
									assignment_id: "assignment_1",
									agent_id: "agent_1",
									occurred_at: "2026-07-18T00:00:01.000Z",
								},
							);
							yield* PersistSurfaceProjection(
								transaction,
								usage("usage_2", "cumulative", 12, 7),
								{
									thread_id: "thread_1",
									run_id: "run_1",
									group_id: "group_1",
									assignment_id: "assignment_1",
									agent_id: "agent_1",
									occurred_at: "2026-07-18T00:00:02.000Z",
								},
							);
							yield* PersistSurfaceProjection(
								transaction,
								usage("usage_3", "delta", 3, 2),
								{
									thread_id: "thread_1",
									run_id: "run_1",
									group_id: "group_1",
									assignment_id: "assignment_1",
									agent_id: "agent_1",
									occurred_at: "2026-07-18T00:00:03.000Z",
								},
							);
							yield* PersistSurfaceProjection(
								transaction,
								usage("usage_3", "delta", 3, 2),
								{
									thread_id: "thread_1",
									run_id: "run_1",
									group_id: "group_1",
									assignment_id: "assignment_1",
									agent_id: "agent_1",
									occurred_at: "2026-07-18T00:00:03.000Z",
								},
							);
						}),
					);
					return yield* database.client.select().from(SurfaceUsageTotals);
				}),
			);
			expect(total).toMatchObject([
				{
					run_id: "run_1",
					group_id: "group_1",
					assignment_id: "assignment_1",
					input_tokens: 15,
					output_tokens: 9,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});
	it("does not interpret an unknown usage basis as a delta", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const [total] = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const persist = (
						observation_id: string,
						basis: string,
						input_tokens?: number,
					) =>
						PersistSurfaceProjection(
							database.client,
							{
								_tag: "usage",
								artisan_run_id: "run_1",
								basis,
								...(input_tokens === undefined ? {} : { input_tokens }),
								observation_id,
								output_tokens: basis === "cumulative" ? 5 : undefined,
								raw: { engine_id: "codex", frame: {}, transport: "test" },
								sequence: Number(observation_id.at(-1)),
							} as any,
							{
								agent_id: "agent_1",
								occurred_at: "2026-07-18T00:00:00.000Z",
								run_id: "run_1",
								thread_id: "thread_1",
							},
						);
					yield* persist("usage_1", "cumulative", 10);
					yield* persist("usage_2", "provider_unknown", 3);
					return yield* database.client.select().from(SurfaceUsageTotals);
				}),
			);
			expect(total).toMatchObject({ input_tokens: null, output_tokens: 5 });
		} finally {
			await runtime.dispose();
		}
	});
	it.each([
		["negative", -1],
		["fractional", 1.5],
		["non-finite", Number.POSITIVE_INFINITY],
	])("returns a typed invariant failure for %s aggregate usage", async (_name, input_tokens) => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const outcome = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const surfaces = yield* SurfaceService;
					yield* database.client.insert(SurfaceUsageTotals).values({
						assignment_id: "assignment_1",
						group_id: "group_1",
						input_tokens,
						output_tokens: 2,
						run_id: "run_1",
						updated_at: "2026-07-18T00:00:00.000Z",
					});
					return yield* Effect.exit(
						surfaces.AggregateUsage({ scope: "group", scope_id: "group_1" }),
					);
				}),
			);
			expect(outcome._tag).toBe("Failure");
			expect(JSON.stringify(outcome)).toContain("SurfaceInvariantFailed");
		} finally {
			await runtime.dispose();
		}
	});
	it("orders surfaces and preserves absent usage without inventing zero", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.insert(SurfaceItems).values([
						{
							surface_id: "surface_2",
							observation_id: "observation_2",
							thread_id: "thread_1",
							run_id: "run_1",
							group_id: null,
							assignment_id: null,
							sequence: 2,
							category: "work",
							kind: "message",
							summary_json: JSON.stringify({ label: "Second" }),
							raw_origin_json: JSON.stringify({
								provider: "codex",
								reference: "two",
							}),
							occurred_at: "2026-07-18T00:00:02.000Z",
						},
						{
							surface_id: "surface_1",
							observation_id: "observation_1",
							thread_id: "thread_1",
							run_id: "run_1",
							group_id: null,
							assignment_id: null,
							sequence: 1,
							category: "work",
							kind: "message",
							summary_json: JSON.stringify({ label: "First" }),
							raw_origin_json: JSON.stringify({
								provider: "codex",
								reference: "one",
							}),
							occurred_at: "2026-07-18T00:00:01.000Z",
						},
					]);
					yield* database.client.insert(SurfaceUsageTotals).values({
						run_id: "run_1",
						group_id: null,
						assignment_id: null,
						input_tokens: null,
						output_tokens: 7,
						updated_at: "2026-07-18T00:00:02.000Z",
					});
					const surfaces = yield* SurfaceService;
					return {
						snapshot: yield* surfaces.List({ thread_id: "thread_1" }),
						usage: yield* surfaces.Usage({ run_id: "run_1" }),
						aggregate: yield* surfaces.AggregateUsage({
							scope: "run",
							scope_id: "run_1",
						}),
					};
				}),
			);
			expect(result.snapshot.items.map((item) => item.surface_id)).toEqual([
				"surface_2",
				"surface_1",
			]);
			expect(result.usage).toEqual([
				expect.objectContaining({ run_id: "run_1", output_tokens: 7 }),
			]);
			expect(result.usage[0]).not.toHaveProperty("input_tokens");
			expect(result.aggregate).toEqual({ scope: "run", scope_id: "run_1", output_tokens: 7 });
		} finally {
			await runtime.dispose();
		}
	});
	it("preserves durable projection interleaving across runs", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const surface_ids = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const surfaces = yield* SurfaceService;
					const insert = (surface_id: string, run_id: string, sequence: number) =>
						database.client.insert(SurfaceItems).values({
							assignment_id: null,
							category: "work",
							group_id: null,
							kind: "message",
							observation_id: `observation_${surface_id}`,
							occurred_at: "2026-07-18T00:00:00.000Z",
							raw_origin_json: JSON.stringify({
								provider: "codex",
								reference: surface_id,
							}),
							run_id,
							sequence,
							summary_json: JSON.stringify({ label: surface_id }),
							surface_id,
							thread_id: "thread_1",
						});
					yield* insert("run_1_first", "run_1", 1);
					yield* insert("run_2_first", "run_2", 1);
					yield* insert("run_1_second", "run_1", 2);
					const snapshot = yield* surfaces.List({ thread_id: "thread_1" });
					return snapshot.items.map((item) => item.surface_id);
				}),
			);
			expect(surface_ids).toEqual(["run_1_first", "run_2_first", "run_1_second"]);
		} finally {
			await runtime.dispose();
		}
	});
	it.each([
		["engine whitespace", "bad engine", "native_1"],
		["engine control", "bad\u0000engine", "native_2"],
		["native whitespace", "codex", "bad native"],
		["native control", "codex", "bad\u0000native"],
		["overlong engine", "x".repeat(4_097), "native_3"],
		["overlong native", "codex", "x".repeat(4_097)],
	])("projects %s provenance as validated opaque work", async (_name, engine_id, native_id) => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					yield* database.client.transaction((transaction) =>
						PersistSurfaceProjection(
							transaction,
							{
								_tag: "native_action",
								action: "provider_event",
								artisan_run_id: "run_1",
								observation_id: "observation_1",
								raw: { engine_id, frame: {}, native_id, transport: "test" },
								sequence: 1,
							} as any,
							{
								agent_id: "agent_1",
								occurred_at: "2026-07-18T00:00:00.000Z",
								run_id: "run_1",
								thread_id: "thread_1",
							},
						),
					);
					const surfaces = yield* SurfaceService;
					return yield* surfaces.List({ thread_id: "thread_1" });
				}),
			);
			expect(snapshot.items).toHaveLength(1);
			expect(snapshot.items[0]).toMatchObject({
				category: "native_action",
				kind: "opaque_engine_work",
				summary: { label: "Opaque engine work" },
			});
			expect(snapshot.items[0]).not.toHaveProperty("raw_origin");
			expect(snapshot.items[0]).not.toHaveProperty("raw_observation");
			expect(JSON.stringify(snapshot)).not.toContain(engine_id);
			expect(JSON.stringify(snapshot)).not.toContain(native_id);
		} finally {
			await runtime.dispose();
		}
	});
	it("preserves unknown metrics while aggregating assignment and group usage", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const surfaces = yield* SurfaceService;
					yield* database.client.insert(SurfaceUsageTotals).values([
						{
							assignment_id: "assignment_1",
							group_id: "group_1",
							input_tokens: 4,
							output_tokens: 2,
							run_id: "run_1",
							updated_at: "2026-07-18T00:00:01.000Z",
						},
						{
							assignment_id: "assignment_1",
							group_id: "group_1",
							input_tokens: 5,
							output_tokens: null,
							run_id: "run_2",
							updated_at: "2026-07-18T00:00:02.000Z",
						},
						{
							assignment_id: "assignment_2",
							group_id: "group_1",
							input_tokens: null,
							output_tokens: 3,
							run_id: "run_3",
							updated_at: "2026-07-18T00:00:03.000Z",
						},
					]);

					return yield* Effect.all({
						assignment: surfaces.AggregateUsage({
							scope: "assignment",
							scope_id: "assignment_1",
						}),
						group: surfaces.AggregateUsage({ scope: "group", scope_id: "group_1" }),
					});
				}),
			);

			expect(result.assignment).toEqual({
				input_tokens: 9,
				scope: "assignment",
				scope_id: "assignment_1",
			});
			expect(result.group).toEqual({ scope: "group", scope_id: "group_1" });
		} finally {
			await runtime.dispose();
		}
	});

	it("rolls run totals into ordered UTC day buckets and drops rows outside the window", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const day_ms = 86_400_000;
			const now_ms = Date.now();
			const iso_at = (days_ago: number) => new Date(now_ms - days_ago * day_ms).toISOString();
			const date_at = (days_ago: number) => iso_at(days_ago).slice(0, 10);
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const surfaces = yield* SurfaceService;
					yield* database.client.insert(SurfaceUsageTotals).values([
						{
							input_tokens: 10,
							output_tokens: 5,
							run_id: "run_today_reported",
							updated_at: iso_at(0),
						},
						{
							/** An unreported metric contributes nothing rather than reading as unknown. */
							input_tokens: 7,
							output_tokens: null,
							run_id: "run_today_partial",
							updated_at: iso_at(0),
						},
						{
							input_tokens: 3,
							output_tokens: 1,
							run_id: "run_yesterday",
							updated_at: iso_at(1),
						},
						{
							input_tokens: 99,
							output_tokens: 99,
							run_id: "run_outside_window",
							updated_at: iso_at(10),
						},
					]);
					yield* database.client.insert(OrchestrationRuns).values({
						agent_id: "agent_daily",
						created_at: iso_at(0),
						engine_id: "claude",
						model_id: "claude-fable-5",
						run_id: "run_today_reported",
						status: "complete",
						thread_id: "thread_daily",
						updated_at: iso_at(0),
						working_directory: tmpdir(),
					});
					yield* database.client.insert(AgentRuns).values({
						agent_id: "agent_daily",
						assignment_id: "assignment_daily",
						attempt: 1,
						created_at: iso_at(0),
						dispatch_status: "completed",
						engine_id: "codex",
						group_id: "group_daily",
						last_observation_sequence: 1,
						model_id: null,
						profile: "default",
						run_id: "run_today_partial",
						state: "complete",
						updated_at: iso_at(0),
					});
					return yield* surfaces.DailyUsageSnapshot({ day_count: 3 });
				}),
			);

			/**
			 * `run_yesterday` has no surviving run row, so its slice carries no engine
			 * id; the codex run predates model stamping, so its slice has no model id.
			 */
			expect(result.buckets).toEqual([
				{
					date: date_at(1),
					engines: [{ input_tokens: 3, output_tokens: 1 }],
					input_tokens: 3,
					output_tokens: 1,
				},
				{
					date: date_at(0),
					engines: [
						{
							engine_id: "claude",
							input_tokens: 10,
							model_id: "claude-fable-5",
							output_tokens: 5,
						},
						{ engine_id: "codex", input_tokens: 7, output_tokens: 0 },
					],
					input_tokens: 17,
					output_tokens: 5,
				},
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("reads surface and usage snapshots at one atomic watermark and filters usage scopes", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					const surfaces = yield* SurfaceService;
					const created = yield* journal.AcceptThreadCreate({
						kind: "command",
						message_id: "surface_snapshot_create",
						origin: "frontend",
						payload: { title: "Surface snapshot", type: "thread.create" },
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-07-18T00:00:00.000Z",
						thread_id: "thread_snapshot",
					});
					yield* database.client.insert(SurfaceUsageTotals).values([
						{
							assignment_id: "assignment_target",
							group_id: "group_target",
							input_tokens: 4,
							output_tokens: 2,
							run_id: "run_target",
							updated_at: "2026-07-18T00:00:01.000Z",
						},
						{
							assignment_id: "assignment_other",
							group_id: "group_other",
							input_tokens: 8,
							output_tokens: 3,
							run_id: "run_other",
							updated_at: "2026-07-18T00:00:02.000Z",
						},
					]);
					return {
						list: yield* surfaces.ListSnapshot({ thread_id: "thread_snapshot" }),
						usage: yield* surfaces.AggregateUsageSnapshot({
							scope: "group",
							scope_id: "group_target",
						}),
						affects: {
							group_target: yield* surfaces.UsageEventAffects(
								{ scope: "group", scope_id: "group_target" },
								"run_target",
							),
							group_other: yield* surfaces.UsageEventAffects(
								{ scope: "group", scope_id: "group_target" },
								"run_other",
							),
							assignment_target: yield* surfaces.UsageEventAffects(
								{ scope: "assignment", scope_id: "assignment_target" },
								"run_target",
							),
							missing_run: yield* surfaces.UsageEventAffects(
								{ scope: "run", scope_id: "run_target" },
								undefined,
							),
						},
						watermark: created.journal_sequence,
					};
				}),
			);
			expect(result.list.journal_sequence).toBe(result.watermark);
			expect(result.usage).toEqual({
				aggregate: {
					input_tokens: 4,
					output_tokens: 2,
					scope: "group",
					scope_id: "group_target",
				},
				journal_sequence: result.watermark,
			});
			expect(result.affects).toEqual({
				assignment_target: true,
				group_other: false,
				group_target: true,
				missing_run: false,
			});
		} finally {
			await runtime.dispose();
		}
	});
});
