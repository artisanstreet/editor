import { describe, expect, it } from "@effect/vitest";
import { Effect, Option } from "effect";

import type { ConversationItem, ConversationTurn } from "@artisan/protocol";
import {
	LatestConversationPlan,
	ThreadChecklist,
	ThreadChecklistLive,
	conversation_plan_has_open_entries,
	type ConversationPlan,
} from "../../modules/frontend/src/lib/conversation/checklist";

const MakePlan = (id: string, ordinal: number, revision = 0): ConversationPlan => ({
	created_at: "2026-08-09T12:00:00.000Z",
	entries: [{ id: `entry-${id}`, state: "pending", text: `Task ${id}` }],
	id,
	lifecycle: "completed",
	ordinal,
	references: [],
	revision,
	source_refs: [],
	state: "active",
	turn_id: "turn-checklist",
	type: "plan",
	updated_at: "2026-08-09T12:00:00.000Z",
});

const MakeTurn = (lifecycle: ConversationTurn["lifecycle"]): ConversationTurn => ({
	created_at: "2026-08-09T12:00:00.000Z",
	id: "turn-checklist",
	lifecycle,
	ordinal: 0,
	references: [],
	revision: 0,
	source_refs: [],
	type: "turn",
	updated_at: "2026-08-09T12:00:00.000Z",
});

describe("thread checklist", () => {
	it("hides a checklist once every entry is crossed out", () => {
		const plan = MakePlan("settled", 1);
		expect(conversation_plan_has_open_entries(plan)).toBe(true);
		expect(
			conversation_plan_has_open_entries({
				...plan,
				entries: [
					{ id: "completed", state: "completed", text: "Done" },
					{ id: "skipped", state: "skipped", text: "Not needed" },
				],
			}),
		).toBe(false);
	});

	it("selects the latest canonical plan and keeps absence explicit", () => {
		expect(Option.isNone(LatestConversationPlan([], []))).toBe(true);

		const selected = LatestConversationPlan(
			[
				MakePlan("older", 3),
				MakePlan("same-ordinal-a", 8),
				MakePlan("same-ordinal-b", 8),
			] satisfies ReadonlyArray<ConversationItem>,
			[MakeTurn("active")],
		);

		expect(Option.getOrThrow(selected).id).toBe("same-ordinal-b");
	});

	it("selects the newest revision when a canonical plan is replaced", () => {
		const selected = LatestConversationPlan(
			[MakePlan("revised", 5, 1), MakePlan("revised", 5, 2)],
			[MakeTurn("active")],
		);

		expect(Option.getOrThrow(selected).revision).toBe(2);
	});

	it("discards a stale active plan when its owning turn has settled", () => {
		const plan = MakePlan("stale", 5);
		expect(Option.isSome(LatestConversationPlan([plan], [MakeTurn("active")]))).toBe(true);
		expect(Option.isNone(LatestConversationPlan([plan], [MakeTurn("completed")]))).toBe(true);
		expect(Option.isNone(LatestConversationPlan([plan], [MakeTurn("failed")]))).toBe(true);
		expect(Option.isNone(LatestConversationPlan([plan], [MakeTurn("cancelled")]))).toBe(true);
	});

	it.effect("keeps a replacement route's checklist when the old lease settles late", () =>
		Effect.gen(function* () {
			const checklist = yield* ThreadChecklist;
			const old_route = yield* checklist.Acquire("thread-old");
			yield* old_route.Publish(Option.some(MakePlan("old", 1)));

			const current_route = yield* checklist.Acquire("thread-current");
			yield* old_route.Publish(Option.some(MakePlan("stale", 2)));
			yield* old_route.Release;
			yield* current_route.Publish(Option.some(MakePlan("current", 3)));

			expect(yield* checklist.Current).toMatchObject({
				_tag: "Ready",
				plan: { id: "current" },
				thread_id: "thread-current",
			});
		}).pipe(Effect.provide(ThreadChecklistLive)),
	);
});
