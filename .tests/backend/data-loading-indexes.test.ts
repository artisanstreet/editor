import { readFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";

const Read = (path: string) => readFileSync(resolve(path), "utf8");
const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: string[] = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-data-loading-indexes-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const journal_schema = Read("modules/backend/src/persistence/schema/journal.ts");
const surfaces_schema = Read("modules/backend/src/persistence/schema/surfaces.ts");
const migration = Read("modules/backend/drizzle/20260814140000_data_loading_indexes/migration.sql");

describe("data-loading indexes", () => {
	it("declares the journal and surface read indexes in the Drizzle schema", () => {
		expect(journal_schema).toContain('index("journal_events_thread_sequence_index")');
		expect(journal_schema).toContain('index("journal_events_thread_run_sequence_index")');
		expect(journal_schema).toContain('index("journal_events_thread_type_sequence_index")');
		expect(surfaces_schema).toContain('index("surface_usage_totals_assignment_id_index")');
		expect(surfaces_schema).toContain('index("surface_usage_totals_group_id_index")');
		expect(surfaces_schema).toContain(
			'index("surface_items_thread_kind_projection_order_index")',
		);
		expect(surfaces_schema).toContain(
			'index("surface_items_thread_run_projection_order_index")',
		);
		expect(surfaces_schema).toContain(
			'index("surface_items_thread_group_projection_order_index")',
		);
	});

	it("reapplies safely and the SQLite planner selects every scoped read index", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const plans = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					for (const statement of migration.split("--> statement-breakpoint")) {
						const sql = statement.trim();
						if (sql !== "") yield* database.client.run(sql);
					}
					const Explain = (query: string) =>
						database.client.all<{ readonly detail: string }>(
							`EXPLAIN QUERY PLAN ${query}`,
						);
					return yield* Effect.all({
						assignment_usage: Explain(
							"SELECT * FROM surface_usage_totals WHERE assignment_id = 'assignment_1'",
						),
						compaction_boundary: Explain(
							"SELECT projection_order FROM surface_items WHERE thread_id = 'thread_1' AND kind = 'compaction' ORDER BY projection_order DESC LIMIT 1",
						),
						group_items: Explain(
							"SELECT * FROM surface_items WHERE thread_id = 'thread_1' AND group_id = 'group_1' ORDER BY projection_order ASC",
						),
						group_usage: Explain(
							"SELECT * FROM surface_usage_totals WHERE group_id = 'group_1'",
						),
						routed_event: Explain(
							"SELECT payload_json FROM journal_events WHERE thread_id = 'thread_1' AND event_type = 'thread.message_routed' ORDER BY sequence DESC LIMIT 1",
						),
						first_target_event: Explain(
							"SELECT sequence FROM journal_events WHERE thread_id = 'thread_1' AND run_id = 'run_1' ORDER BY sequence ASC LIMIT 1",
						),
						thread_events: Explain(
							"SELECT * FROM journal_events WHERE thread_id = 'thread_1' AND sequence > 10 ORDER BY sequence ASC LIMIT 200",
						),
						run_items: Explain(
							"SELECT * FROM surface_items WHERE thread_id = 'thread_1' AND run_id = 'run_1' ORDER BY projection_order ASC",
						),
					});
				}),
			);

			const uses = (plan: ReadonlyArray<{ readonly detail: string }>, index_name: string) =>
				plan.some((row) => row.detail.includes(index_name));
			expect(uses(plans.thread_events, "journal_events_thread_sequence_index")).toBe(true);
			expect(uses(plans.first_target_event, "journal_events_thread_run_sequence_index")).toBe(
				true,
			);
			expect(uses(plans.routed_event, "journal_events_thread_type_sequence_index")).toBe(
				true,
			);
			expect(uses(plans.assignment_usage, "surface_usage_totals_assignment_id_index")).toBe(
				true,
			);
			expect(uses(plans.group_usage, "surface_usage_totals_group_id_index")).toBe(true);
			expect(
				uses(plans.compaction_boundary, "surface_items_thread_kind_projection_order_index"),
			).toBe(true);
			expect(uses(plans.run_items, "surface_items_thread_run_projection_order_index")).toBe(
				true,
			);
			expect(
				uses(plans.group_items, "surface_items_thread_group_projection_order_index"),
			).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});
});
