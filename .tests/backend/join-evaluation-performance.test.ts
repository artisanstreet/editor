import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import type { EventEnvelope } from "@artisan/protocol";

import { Assignments, OrchestrationJoins } from "../../modules/backend/src/persistence/tables";
import { make_join_evaluation } from "../../modules/backend/src/orchestration/internal/join-evaluation";
import type {
	GraphContext,
	GraphTransaction,
	GraphTransitionInput,
} from "../../modules/backend/src/orchestration/internal/graph-context";
import type { GraphLedger } from "../../modules/backend/src/orchestration/internal/graph-ledger";
import { make_persisted_graph_codecs } from "../../modules/backend/src/orchestration/internal/persisted-graph-codecs";

const input: GraphTransitionInput = {
	causation_id: "cause_join",
	correlation_id: "correlation_join",
	group_id: "group_join",
	thread_id: "thread_join",
};

function joining_row(
	join_id: string,
	strategy: "first_success" | "require_all",
	upstream_assignment_ids: ReadonlyArray<string>,
) {
	return {
		created_at: "2026-08-15T10:00:00.000Z",
		downstream_assignment_id: null,
		group_id: input.group_id,
		join_id,
		selected_assignment_id: null,
		state: "joining",
		strategy,
		updated_at: "2026-08-15T10:00:00.000Z",
		upstream_assignment_ids_json: JSON.stringify(upstream_assignment_ids),
	} as typeof OrchestrationJoins.$inferSelect;
}

function make_harness(
	joins: ReadonlyArray<typeof OrchestrationJoins.$inferSelect>,
	assignments: ReadonlyArray<{ readonly assignment_id: string; readonly state: string }>,
) {
	let assignment_selects = 0;
	const updates: Array<Record<string, unknown>> = [];
	const transaction = {
		select: (_selection?: unknown) => ({
			from: (table: unknown) => {
				if (table === OrchestrationJoins) {
					return { where: () => Effect.succeed(joins) };
				}
				if (table === Assignments) {
					assignment_selects += 1;
					return {
						where: () => ({ orderBy: () => Effect.succeed(assignments) }),
					};
				}

				throw new Error("Unexpected table");
			},
		}),
		update: (table: unknown) => {
			if (table !== OrchestrationJoins) throw new Error("Unexpected update table");

			return {
				set: (values: Record<string, unknown>) => ({
					where: () => Effect.sync(() => void updates.push(values)),
				}),
			};
		},
	} as unknown as GraphTransaction;
	const context = {
		metadata: { Now: Effect.succeed("2026-08-15T10:01:00.000Z") },
	} as GraphContext;
	const ledger = {
		append_event: (...[_transaction, event]: Parameters<GraphLedger["append_event"]>) =>
			Effect.succeed({ payload: event.payload } as EventEnvelope),
	} as unknown as GraphLedger;
	const evaluation = make_join_evaluation(
		context,
		make_persisted_graph_codecs({} as GraphContext),
		ledger,
	);

	return {
		assignment_selects: () => assignment_selects,
		evaluation,
		transaction,
		updates,
	};
}

describe("join evaluation assignment batching", () => {
	it("shares one assignment query across overlapping joins while retaining first-success order", async () => {
		const harness = make_harness(
			[
				joining_row("join_first", "first_success", ["assignment_z", "assignment_a"]),
				joining_row("join_all", "require_all", ["assignment_a", "assignment_b"]),
			],
			[
				{ assignment_id: "assignment_a", state: "complete" },
				{ assignment_id: "assignment_b", state: "complete" },
				{ assignment_id: "assignment_z", state: "complete" },
			],
		);

		const events = await Effect.runPromise(
			harness.evaluation.resolve_joins(harness.transaction, input),
		);

		expect(harness.assignment_selects()).toBe(1);
		expect(harness.updates).toEqual([
			{
				selected_assignment_id: "assignment_a",
				state: "complete",
				updated_at: "2026-08-15T10:01:00.000Z",
			},
			{
				selected_assignment_id: null,
				state: "complete",
				updated_at: "2026-08-15T10:01:00.000Z",
			},
		]);
		expect(
			events.map(({ payload }) =>
				payload.type === "orchestration.graph.lifecycle" ? payload.node_id : undefined,
			),
		).toEqual(["join_first", "join_all"]);
	});

	it("preserves missing and duplicate assignment validation after the shared query", async () => {
		for (const upstream_assignment_ids of [
			["assignment_a", "assignment_missing"],
			["assignment_a", "assignment_a"],
		]) {
			const harness = make_harness(
				[joining_row("join_invalid", "require_all", upstream_assignment_ids)],
				[{ assignment_id: "assignment_a", state: "complete" }],
			);

			await expect(
				Effect.runPromise(harness.evaluation.resolve_joins(harness.transaction, input)),
			).rejects.toMatchObject({
				_tag: "AgentGraphInvalid",
				message: "Join join_invalid lost an upstream assignment",
			});
			expect(harness.assignment_selects()).toBe(1);
			expect(harness.updates).toEqual([]);
		}
	});

	it("preserves row-order error precedence around malformed later joins", async () => {
		const malformed = {
			...joining_row("join_malformed", "require_all", ["unused"]),
			upstream_assignment_ids_json: "{not-json",
		};
		const earlier_missing = make_harness(
			[
				joining_row("join_missing_first", "require_all", [
					"assignment_a",
					"assignment_missing",
				]),
				malformed,
			],
			[{ assignment_id: "assignment_a", state: "complete" }],
		);

		await expect(
			Effect.runPromise(
				earlier_missing.evaluation.resolve_joins(earlier_missing.transaction, input),
			),
		).rejects.toMatchObject({
			_tag: "AgentGraphInvalid",
			message: "Join join_missing_first lost an upstream assignment",
		});
		expect(earlier_missing.assignment_selects()).toBe(1);

		const malformed_first = make_harness([malformed], []);
		await expect(
			Effect.runPromise(
				malformed_first.evaluation.resolve_joins(malformed_first.transaction, input),
			),
		).rejects.toBeDefined();
		expect(malformed_first.assignment_selects()).toBe(0);
	});

	it("does not query assignments when there are no joining rows", async () => {
		const harness = make_harness([], []);

		await expect(
			Effect.runPromise(harness.evaluation.resolve_joins(harness.transaction, input)),
		).resolves.toEqual([]);
		expect(harness.assignment_selects()).toBe(0);
	});
});
