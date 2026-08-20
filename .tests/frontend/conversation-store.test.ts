import { describe, expect, it, vi } from "vitest";
import { Schema } from "effect";
import { ConversationPatch, ConversationSnapshot } from "@artisan/protocol";
import {
	ApplyConversationViewPatch,
	CanReplaceConversationSnapshot,
	MakeConversationRenderBlocks,
	MakeConversationRenderWindow,
	MakeParticipantConversationRenderWindow,
	MakeConversationViewState,
} from "../../modules/frontend/src/lib/conversation/store";
import { MakeMockConversation } from "../../modules/frontend/src/lib/conversation/mock";
import { make_conversation_trace_segments } from "../../modules/frontend/src/lib/conversation/trace";

const snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
	conversation_id: "conversation-test",
	items: [
		{
			created_at: "2026-07-24T12:00:00.000Z",
			id: "item-a",
			lifecycle: "streaming",
			ordinal: 1,
			references: [],
			revision: 0,
			source_refs: [],
			phase: "unspecified",
			text: "Hello",
			turn_id: "turn-a",
			type: "assistant_message",
			updated_at: "2026-07-24T12:00:00.000Z",
		},
	],
	journal_sequence: 0,
	last_patch_sequence: 0,
	schema_version: 1,
	thread_id: "thread-test",
	turns: [
		{
			created_at: "2026-07-24T12:00:00.000Z",
			id: "turn-a",
			lifecycle: "streaming",
			ordinal: 0,
			references: [],
			revision: 0,
			source_refs: [],
			type: "turn",
			updated_at: "2026-07-24T12:00:00.000Z",
		},
	],
	updated_at: "2026-07-24T12:00:00.000Z",
});
const patch = (value: unknown) => Schema.decodeUnknownSync(ConversationPatch)(value);

describe("conversation view store", () => {
	it("isolates root and adopted child turns while preserving child parent references", () => {
		const participant_snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			...snapshot,
			items: [
				{
					...snapshot.items[0],
					id: "root-message",
					lifecycle: "completed",
					phase: "final",
					turn_id: "turn-a",
				},
				{
					...snapshot.items[0],
					id: "child-message",
					lifecycle: "completed",
					ordinal: 3,
					phase: "final",
					text: "Child",
					turn_id: "turn-child",
				},
			],
			turns: [
				{ ...snapshot.turns[0], lifecycle: "completed" },
				{
					...snapshot.turns[0],
					agent_id: "agent-noodle",
					id: "turn-child",
					lifecycle: "completed",
					ordinal: 2,
					parent_id: "turn-a",
				},
			],
		});
		const initialized = MakeConversationViewState(participant_snapshot);
		if (initialized._tag !== "applied") throw new Error("fixture must initialize");

		expect([
			...new Set(
				MakeParticipantConversationRenderWindow(
					initialized.state,
					undefined,
					24,
				).blocks.map((block) => block.turn_id),
			),
		]).toEqual(["turn-a"]);
		expect([
			...new Set(
				MakeParticipantConversationRenderWindow(
					initialized.state,
					"agent-noodle",
					24,
				).blocks.map((block) => block.turn_id),
			),
		]).toEqual(["turn-child"]);
	});

	it("keeps plan items in canonical state but out of transcript render blocks", () => {
		const plan_snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			...snapshot,
			items: [
				{
					created_at: "2026-07-24T12:00:00.000Z",
					entries: [{ id: "task-a", state: "active", text: "Move the plan" }],
					id: "plan-a",
					lifecycle: "streaming",
					ordinal: 1,
					references: [],
					revision: 0,
					source_refs: [],
					state: "active",
					turn_id: "turn-a",
					type: "plan",
					updated_at: "2026-07-24T12:00:00.000Z",
				},
			],
		});
		const initialized = MakeConversationViewState(plan_snapshot);
		if (initialized._tag !== "applied") throw new Error("fixture must initialize");

		expect(initialized.state.items_by_id.get("plan-a")?.type).toBe("plan");
		expect(MakeConversationRenderBlocks(initialized.state)).toEqual([]);
	});

	it("projects an exact, bounded newest-group window from large history", () => {
		const large_snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-window",
			items: Array.from({ length: 100 }, (_, index) => ({
				created_at: "2026-07-24T12:00:00.000Z",
				id: `item-window-${index}`,
				lifecycle: "completed",
				ordinal: index * 2 + 1,
				phase: "final",
				references: [],
				revision: 0,
				source_refs: [],
				text: `Message ${index}`,
				turn_id: `turn-window-${index}`,
				type: "assistant_message",
				updated_at: "2026-07-24T12:00:00.000Z",
			})),
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-window",
			turns: Array.from({ length: 100 }, (_, index) => ({
				created_at: "2026-07-24T12:00:00.000Z",
				id: `turn-window-${index}`,
				lifecycle: "completed",
				ordinal: index * 2,
				references: [],
				revision: 0,
				source_refs: [],
				type: "turn",
				updated_at: "2026-07-24T12:00:00.000Z",
			})),
			updated_at: "2026-07-24T12:00:00.000Z",
		});
		const initialized = MakeConversationViewState(large_snapshot);
		if (initialized._tag !== "applied") throw new Error("fixture must initialize");

		const newest = MakeConversationRenderWindow(initialized.state, 24);
		expect(newest.hidden_group_count).toBe(76);
		expect(newest.blocks).toHaveLength(48);
		expect([...new Set(newest.blocks.map((block) => block.turn_id))]).toEqual(
			Array.from({ length: 24 }, (_, index) => `turn-window-${index + 76}`),
		);

		const paged = MakeConversationRenderWindow(initialized.state, 24, 24);
		expect(paged.hidden_group_count).toBe(52);
		expect(paged.blocks).toHaveLength(96);
		expect(paged.blocks.at(0)).toMatchObject({ turn_id: "turn-window-52" });
		expect(paged.blocks.at(-1)).toMatchObject({ turn_id: "turn-window-99" });
	});

	it("preserves item identity while keeping ordinal ordering", () => {
		const initial = MakeConversationViewState(snapshot);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
		const original = initial.state.items_by_id.get("item-a");
		const result = ApplyConversationViewPatch(
			initial.state,
			patch({
				patch_id: "patch-upsert",
				sequence: 1,
				item: {
					created_at: "2026-07-24T12:00:00.000Z",
					id: "item-b",
					lifecycle: "completed",
					ordinal: 2,
					references: [],
					revision: 0,
					source_refs: [],
					kind: "read",
					label: "Read",
					status: "completed",
					turn_id: "turn-a",
					type: "activity",
					updated_at: "2026-07-24T12:00:00.000Z",
				},
				type: "item_upsert",
			}),
		);
		expect(result._tag).toBe("applied");
		if (result._tag !== "applied") return;
		expect(result.state.items_by_id.get("item-a")).toBe(original);
		expect(result.state.ordered_item_ids).toEqual(["item-a", "item-b"]);
	});

	it("refreshes the newest cached group for streamed item and turn patches", () => {
		const initial = MakeConversationViewState(snapshot);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
		const appended = ApplyConversationViewPatch(
			initial.state,
			patch({
				item_id: "item-a",
				patch_id: "window-append",
				revision: 1,
				sequence: 1,
				text: " world",
				type: "item_append",
			}),
		);
		if (appended._tag !== "applied") throw new Error("append must apply");
		expect(MakeConversationRenderWindow(appended.state, 24).blocks).toEqual([
			expect.objectContaining({ item: expect.objectContaining({ text: "Hello world" }) }),
		]);
		const completed = ApplyConversationViewPatch(
			appended.state,
			patch({
				lifecycle: "completed",
				patch_id: "window-turn-complete",
				revision: 1,
				sequence: 2,
				turn_id: "turn-a",
				type: "turn_lifecycle",
			}),
		);
		if (completed._tag !== "applied") throw new Error("turn completion must apply");
		expect(MakeConversationRenderWindow(completed.state, 24).blocks).toHaveLength(1);
	});

	it("patches a growing active work group without sorting or regenerating its historical details", () => {
		const detail_count = 512;
		const active_snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-active-group",
			items: [
				{
					created_at: "2026-07-24T12:00:00.000Z",
					ended_at: undefined,
					id: "work-session",
					lifecycle: "streaming",
					ordinal: 1,
					references: [],
					revision: 0,
					responded_at: undefined,
					run_id: "run-active-group",
					source_refs: [],
					started_at: "2026-07-24T12:00:00.000Z",
					status: "streaming",
					title: "Working",
					turn_id: "turn-active-group",
					type: "work_session",
					updated_at: "2026-07-24T12:00:00.000Z",
				},
				...Array.from({ length: detail_count }, (_, index) => ({
					created_at: "2026-07-24T12:00:00.000Z",
					id: `activity-${index}`,
					kind: "tool",
					label: `Activity ${index}`,
					lifecycle: "streaming",
					ordinal: index + 2,
					references: [],
					revision: 0,
					source_refs: [],
					status: "streaming",
					turn_id: "turn-active-group",
					type: "activity",
					updated_at: "2026-07-24T12:00:00.000Z",
				})),
				{
					created_at: "2026-07-24T12:00:00.000Z",
					id: "streaming-message",
					lifecycle: "streaming",
					ordinal: detail_count + 2,
					phase: "unspecified",
					references: [],
					revision: 0,
					source_refs: [],
					text: "",
					turn_id: "turn-active-group",
					type: "assistant_message",
					updated_at: "2026-07-24T12:00:00.000Z",
				},
			],
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-active-group",
			turns: [
				{
					created_at: "2026-07-24T12:00:00.000Z",
					id: "turn-active-group",
					lifecycle: "streaming",
					ordinal: 0,
					references: [],
					revision: 0,
					source_refs: [],
					type: "turn",
					updated_at: "2026-07-24T12:00:00.000Z",
				},
			],
			updated_at: "2026-07-24T12:00:00.000Z",
		});
		const initialized = MakeConversationViewState(active_snapshot);
		if (initialized._tag !== "applied") throw new Error("fixture must initialize");
		const sort = vi.spyOn(Array.prototype, "sort");
		let state = initialized.state;
		for (let sequence = 1; sequence <= 64; sequence += 1) {
			const result = ApplyConversationViewPatch(
				state,
				patch({
					item_id: "streaming-message",
					patch_id: `active-group-append-${sequence}`,
					revision: sequence,
					sequence,
					text: ".",
					type: "item_append",
				}),
			);
			if (result._tag !== "applied") throw new Error("append must apply");
			state = result.state;
		}
		const lifecycle = ApplyConversationViewPatch(
			state,
			patch({
				item_id: "streaming-message",
				lifecycle: "active",
				patch_id: "active-group-lifecycle",
				revision: 65,
				sequence: 65,
				type: "item_lifecycle",
			}),
		);
		expect(sort).not.toHaveBeenCalled();
		sort.mockRestore();
		if (lifecycle._tag !== "applied") throw new Error("lifecycle must apply");
		const work_group = MakeConversationRenderWindow(lifecycle.state, 24).blocks.find(
			(block) => block.type === "work_group",
		);
		expect(work_group).toMatchObject({
			details: expect.arrayContaining([
				expect.objectContaining({ id: "activity-0" }),
				expect.objectContaining({
					id: "streaming-message",
					lifecycle: "active",
					text: ".".repeat(64),
				}),
			]),
		});
	});

	it("appends and completes the same streaming message", () => {
		const initial = MakeConversationViewState(snapshot);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
		const items_by_id = initial.state.items_by_id;
		const ordered_item_ids = initial.state.ordered_item_ids;
		const appended = ApplyConversationViewPatch(
			initial.state,
			patch({
				patch_id: "patch-append",
				sequence: 1,
				item_id: "item-a",
				revision: 1,
				text: " world",
				type: "item_append",
			}),
		);
		if (appended._tag !== "applied") throw new Error("append must apply");
		expect(appended.state).not.toBe(initial.state);
		expect(appended.state.items_by_id).toBe(items_by_id);
		expect(appended.state.ordered_item_ids).toBe(ordered_item_ids);
		expect(initial.state.items_by_id.get("item-a")).toMatchObject({ text: "Hello world" });
		const completed = ApplyConversationViewPatch(
			appended.state,
			patch({
				patch_id: "patch-complete",
				sequence: 2,
				item_id: "item-a",
				lifecycle: "completed",
				revision: 2,
				type: "item_lifecycle",
			}),
		);
		expect(completed._tag).toBe("applied");
		if (completed._tag !== "applied") return;
		expect(completed.state.items_by_id).toBe(items_by_id);
		expect(completed.state.ordered_item_ids).toBe(ordered_item_ids);
		expect(completed.state.items_by_id.get("item-a")).toMatchObject({
			text: "Hello world",
			lifecycle: "completed",
		});
	});

	it("inserts a new item by ordinal while preserving the existing item index", () => {
		const initial = MakeConversationViewState(
			Schema.decodeUnknownSync(ConversationSnapshot)({
				...snapshot,
				items: [
					{ ...snapshot.items[0]!, ordinal: 3 },
					{
						...snapshot.items[0]!,
						id: "item-c",
						ordinal: 5,
						text: "Later",
					},
				],
			}),
		);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
		const items_by_id = initial.state.items_by_id;
		const result = ApplyConversationViewPatch(
			initial.state,
			patch({
				patch_id: "patch-middle-upsert",
				sequence: 1,
				item: {
					created_at: "2026-07-24T12:00:00.000Z",
					id: "item-b",
					lifecycle: "completed",
					ordinal: 4,
					references: [],
					revision: 0,
					source_refs: [],
					kind: "read",
					label: "Read",
					status: "completed",
					turn_id: "turn-a",
					type: "activity",
					updated_at: "2026-07-24T12:00:00.000Z",
				},
				type: "item_upsert",
			}),
		);

		expect(result._tag).toBe("applied");
		if (result._tag !== "applied") return;
		expect(result.state.items_by_id).toBe(items_by_id);
		expect(result.state.ordered_item_ids).toEqual(["item-a", "item-b", "item-c"]);
	});

	it("establishes a new render group without disturbing existing group order", () => {
		const initial = MakeConversationViewState(snapshot);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
		const with_turn = ApplyConversationViewPatch(
			initial.state,
			patch({
				patch_id: "patch-new-turn",
				sequence: 1,
				turn: {
					created_at: "2026-07-24T12:00:01.000Z",
					id: "turn-b",
					lifecycle: "streaming",
					ordinal: 2,
					references: [],
					revision: 0,
					source_refs: [],
					type: "turn",
					updated_at: "2026-07-24T12:00:01.000Z",
				},
				type: "turn_upsert",
			}),
		);
		if (with_turn._tag !== "applied") throw new Error("turn must apply");
		const with_item = ApplyConversationViewPatch(
			with_turn.state,
			patch({
				item: {
					created_at: "2026-07-24T12:00:01.000Z",
					id: "item-b",
					lifecycle: "streaming",
					ordinal: 3,
					phase: "unspecified",
					references: [],
					revision: 0,
					source_refs: [],
					text: "Later",
					turn_id: "turn-b",
					type: "assistant_message",
					updated_at: "2026-07-24T12:00:01.000Z",
				},
				patch_id: "patch-new-item",
				sequence: 2,
				type: "item_upsert",
			}),
		);
		if (with_item._tag !== "applied") throw new Error("item must apply");

		expect(
			MakeConversationRenderWindow(with_item.state, 24).blocks.map((block) => block.turn_id),
		).toEqual(["turn-a", "turn-b"]);
	});

	it("keeps renderer item collections unchanged for turn patches and duplicate or invalid patches", () => {
		const initial = MakeConversationViewState(snapshot);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
		const items_by_id = initial.state.items_by_id;
		const ordered_item_ids = initial.state.ordered_item_ids;
		const applied = ApplyConversationViewPatch(
			initial.state,
			patch({
				patch_id: "patch-turn-lifecycle",
				sequence: 1,
				lifecycle: "active",
				revision: 1,
				turn_id: "turn-a",
				type: "turn_lifecycle",
			}),
		);
		if (applied._tag !== "applied") throw new Error("turn patch must apply");
		expect(applied.state.items_by_id).toBe(items_by_id);
		expect(applied.state.ordered_item_ids).toBe(ordered_item_ids);

		const duplicate = ApplyConversationViewPatch(
			applied.state,
			patch({
				patch_id: "patch-turn-lifecycle",
				sequence: 2,
				lifecycle: "completed",
				revision: 2,
				turn_id: "turn-a",
				type: "turn_lifecycle",
			}),
		);
		expect(duplicate).toMatchObject({ _tag: "duplicate", state: applied.state });
		expect(applied.state.items_by_id).toBe(items_by_id);
		expect(applied.state.ordered_item_ids).toBe(ordered_item_ids);

		const invalid = ApplyConversationViewPatch(
			applied.state,
			patch({
				patch_id: "patch-invalid-item-lifecycle",
				sequence: 2,
				item_id: "item-a",
				lifecycle: "completed",
				revision: 0,
				type: "item_lifecycle",
			}),
		);
		expect(invalid._tag).toBe("invariant_error");
		expect(applied.state.items_by_id).toBe(items_by_id);
		expect(applied.state.items_by_id.get("item-a")).toMatchObject({ revision: 0 });
	});

	it("rejects a delayed resync snapshot after the live projection has advanced", () => {
		const initial = MakeConversationViewState(snapshot);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
		const advanced = ApplyConversationViewPatch(
			initial.state,
			patch({
				patch_id: "patch-live-before-resync",
				sequence: 1,
				item_id: "item-a",
				revision: 1,
				text: " streamed",
				type: "item_append",
			}),
		);
		if (advanced._tag !== "applied") throw new Error("live patch must apply");

		expect(CanReplaceConversationSnapshot(advanced.state.rebuild.snapshot, snapshot)).toBe(
			false,
		);
		expect(advanced.state.items_by_id.get("item-a")).toMatchObject({ text: "Hello streamed" });
		expect(
			CanReplaceConversationSnapshot(
				advanced.state.rebuild.snapshot,
				advanced.state.rebuild.snapshot,
			),
		).toBe(true);
	});

	it("ignores duplicate patches and requests resync after a gap", () => {
		const initial = MakeConversationViewState(snapshot);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
		const applied = ApplyConversationViewPatch(
			initial.state,
			patch({
				patch_id: "patch-append",
				sequence: 1,
				item_id: "item-a",
				revision: 1,
				text: "!",
				type: "item_append",
			}),
		);
		if (applied._tag !== "applied") throw new Error("patch must apply");
		expect(
			ApplyConversationViewPatch(
				applied.state,
				patch({
					patch_id: "patch-append",
					sequence: 2,
					item_id: "item-a",
					revision: 2,
					text: "?",
					type: "item_append",
				}),
			)._tag,
		).toBe("duplicate");
		const gap = ApplyConversationViewPatch(
			applied.state,
			patch({
				patch_id: "patch-gap",
				sequence: 3,
				item_id: "item-a",
				revision: 2,
				text: "?",
				type: "item_append",
			}),
		);
		expect(gap).toMatchObject({
			_tag: "resync_required",
			expected_sequence: 2,
			received_sequence: 3,
		});
	});

	it("collapses typed intermediate work without swallowing the final reply", () => {
		const initial = MakeConversationViewState(MakeMockConversation("grouping-test"));
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const first_group = blocks.find(
			(block) => block.type === "work_group" && block.session.id === "mock-work-1",
		);

		expect(first_group).toMatchObject({ type: "work_group" });
		if (first_group?.type === "work_group") {
			expect(first_group.duration_kind).toBe("worked");
			expect(first_group.details).toContainEqual(
				expect.objectContaining({
					id: "mock-reasoning-1",
					type: "reasoning_summary",
				}),
			);
		}
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "mock-assistant-1"),
		).toBe(true);
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "mock-reasoning-1"),
		).toBe(false);
	});

	it("folds a run's engine handoff into its work group", () => {
		const handoff = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-handoff",
			items: [
				{
					continuation: "portable",
					created_at: "2026-07-30T12:00:00.000Z",
					id: "transition-hosted",
					lifecycle: "completed",
					ordinal: 1,
					references: [],
					revision: 0,
					source_engine_id: "claude",
					source_refs: [],
					target_engine_id: "codex",
					turn_id: "turn-handoff",
					state: "completed",
					type: "model_transition",
					updated_at: "2026-07-30T12:00:00.000Z",
				},
				{
					created_at: "2026-07-30T12:00:01.000Z",
					ended_at: "2026-07-30T12:00:09.000Z",
					id: "work-handoff",
					lifecycle: "completed",
					ordinal: 2,
					references: [],
					revision: 1,
					source_refs: [],
					started_at: "2026-07-30T12:00:01.000Z",
					status: "completed",
					title: "Agent work",
					turn_id: "turn-handoff",
					type: "work_session",
					updated_at: "2026-07-30T12:00:09.000Z",
				},
				{
					continuation: "portable",
					created_at: "2026-07-30T12:00:10.000Z",
					id: "transition-orphan",
					lifecycle: "completed",
					ordinal: 3,
					references: [],
					revision: 0,
					source_engine_id: "codex",
					source_refs: [],
					target_engine_id: "claude",
					turn_id: "turn-orphan",
					state: "completed",
					type: "model_transition",
					updated_at: "2026-07-30T12:00:10.000Z",
				},
			],
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-handoff",
			turns: [
				{
					created_at: "2026-07-30T12:00:00.000Z",
					id: "turn-handoff",
					lifecycle: "completed",
					ordinal: 0,
					references: [],
					revision: 1,
					source_refs: [],
					type: "turn",
					updated_at: "2026-07-30T12:00:09.000Z",
				},
				{
					created_at: "2026-07-30T12:00:10.000Z",
					id: "turn-orphan",
					lifecycle: "completed",
					ordinal: 4,
					references: [],
					revision: 0,
					source_refs: [],
					type: "turn",
					updated_at: "2026-07-30T12:00:10.000Z",
				},
			],
			updated_at: "2026-07-30T12:00:10.000Z",
		});
		const initial = MakeConversationViewState(handoff);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const work = blocks.find(
			(block) => block.type === "work_group" && block.session.id === "work-handoff",
		);
		if (work?.type !== "work_group") throw new Error("work must render");

		expect(work.transition).toMatchObject({ id: "transition-hosted" });
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "transition-hosted"),
		).toBe(false);
		/** A handoff whose turn produced no session keeps its own timeline row. */
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "transition-orphan"),
		).toBe(true);
	});

	it("keeps commentary interleaved with activity inside completed work", () => {
		const completed_at = "2026-07-26T12:00:06.000Z";
		const interleaved = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-interleaved-work",
			items: [
				{
					created_at: "2026-07-26T12:00:00.000Z",
					ended_at: completed_at,
					id: "work-interleaved",
					lifecycle: "completed",
					ordinal: 1,
					references: [],
					revision: 1,
					source_refs: [],
					started_at: "2026-07-26T12:00:00.000Z",
					status: "completed",
					title: "Agent work",
					turn_id: "turn-interleaved",
					type: "work_session",
					updated_at: completed_at,
				},
				{
					created_at: "2026-07-26T12:00:01.000Z",
					id: "commentary-1",
					lifecycle: "completed",
					ordinal: 2,
					phase: "commentary",
					references: [],
					revision: 0,
					source_refs: [],
					text: "Sentence 1",
					turn_id: "turn-interleaved",
					type: "assistant_message",
					updated_at: "2026-07-26T12:00:01.000Z",
				},
				{
					created_at: "2026-07-26T12:00:02.000Z",
					id: "activity-1",
					kind: "terminal_activity",
					label: "Ran command one",
					lifecycle: "completed",
					ordinal: 3,
					references: [],
					revision: 0,
					source_refs: [],
					status: "completed",
					turn_id: "turn-interleaved",
					type: "activity",
					updated_at: "2026-07-26T12:00:02.000Z",
				},
				{
					created_at: "2026-07-26T12:00:03.000Z",
					id: "commentary-2",
					lifecycle: "completed",
					ordinal: 4,
					phase: "commentary",
					references: [],
					revision: 0,
					source_refs: [],
					text: "Sentence 2",
					turn_id: "turn-interleaved",
					type: "assistant_message",
					updated_at: "2026-07-26T12:00:03.000Z",
				},
				{
					created_at: "2026-07-26T12:00:04.000Z",
					id: "activity-2",
					kind: "search",
					label: "Searched docs",
					lifecycle: "completed",
					ordinal: 5,
					references: [],
					revision: 0,
					source_refs: [],
					status: "completed",
					turn_id: "turn-interleaved",
					type: "activity",
					updated_at: "2026-07-26T12:00:04.000Z",
				},
				{
					created_at: "2026-07-26T12:00:05.000Z",
					id: "final-interleaved",
					lifecycle: "completed",
					ordinal: 6,
					phase: "final",
					references: [],
					revision: 0,
					source_refs: [],
					text: "Final answer",
					turn_id: "turn-interleaved",
					type: "assistant_message",
					updated_at: "2026-07-26T12:00:05.000Z",
				},
			],
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-interleaved-work",
			turns: [
				{
					created_at: "2026-07-26T12:00:00.000Z",
					id: "turn-interleaved",
					lifecycle: "completed",
					ordinal: 0,
					references: [],
					revision: 1,
					source_refs: [],
					type: "turn",
					updated_at: completed_at,
				},
			],
			updated_at: completed_at,
		});
		const initial = MakeConversationViewState(interleaved);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const work = blocks.find(
			(block) => block.type === "work_group" && block.session.id === "work-interleaved",
		);
		if (work?.type !== "work_group") throw new Error("work must render");

		expect(work.details.map((item) => item.id)).toEqual([
			"commentary-1",
			"activity-1",
			"commentary-2",
			"activity-2",
		]);
		expect(
			blocks.some(
				(block) =>
					block.type === "item" &&
					(block.item.id === "commentary-1" || block.item.id === "commentary-2"),
			),
		).toBe(false);
		expect(
			make_conversation_trace_segments(work.details, false).map((segment) => ({
				id: segment.id,
				type: segment.type,
			})),
		).toEqual([
			{ id: "commentary-1", type: "item" },
			{ id: "activities:activity-1", type: "activity_group" },
			{ id: "commentary-2", type: "item" },
			{ id: "activities:activity-2", type: "activity_group" },
		]);
		const work_index = blocks.findIndex((block) => block === work);
		const final_index = blocks.findIndex(
			(block) => block.type === "item" && block.item.id === "final-interleaved",
		);
		expect(final_index).toBeGreaterThan(work_index);
	});

	it("keeps legacy unspecified and mislabeled-final progress inside completed work", () => {
		const completed_at = "2026-07-26T12:00:04.000Z";
		const base_item = {
			created_at: "2026-07-26T12:00:00.000Z",
			lifecycle: "completed",
			references: [],
			revision: 0,
			source_refs: [],
			turn_id: "turn-legacy-progress",
			updated_at: completed_at,
		};
		const legacy_progress = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-legacy-progress",
			items: [
				{
					...base_item,
					ended_at: completed_at,
					id: "work-legacy-progress",
					ordinal: 1,
					revision: 1,
					started_at: "2026-07-26T12:00:00.000Z",
					status: "completed",
					title: "Agent work",
					type: "work_session",
				},
				{
					...base_item,
					id: "unspecified-progress",
					ordinal: 2,
					phase: "unspecified",
					text: "Progress update",
					type: "assistant_message",
				},
				{
					...base_item,
					id: "legacy-activity",
					kind: "terminal_activity",
					label: "Ran a command",
					ordinal: 3,
					status: "completed",
					type: "activity",
				},
				{
					...base_item,
					id: "unspecified-final",
					ordinal: 4,
					phase: "unspecified",
					text: "Settled answer",
					type: "assistant_message",
				},
			],
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-legacy-progress",
			turns: [
				{
					...base_item,
					id: "turn-legacy-progress",
					ordinal: 0,
					revision: 1,
					type: "turn",
				},
			],
			updated_at: completed_at,
		});
		const initial = MakeConversationViewState(legacy_progress);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const work = blocks.find((block) => block.type === "work_group");
		if (work?.type !== "work_group") throw new Error("work must render");
		expect(work.details.map((item) => item.id)).toEqual([
			"unspecified-progress",
			"legacy-activity",
		]);
		expect(
			blocks.find((block) => block.type === "item" && block.item.id === "unspecified-final"),
		).toBeDefined();

		const historical = Schema.decodeUnknownSync(ConversationSnapshot)({
			...legacy_progress,
			conversation_id: "conversation-mislabeled-finals",
			items: legacy_progress.items.map((item) =>
				item.type === "assistant_message" ? { ...item, phase: "final" } : item,
			),
		});
		const historical_initial = MakeConversationViewState(historical);
		if (historical_initial._tag !== "applied") throw new Error("fixture must initialize");
		const historical_blocks = MakeConversationRenderBlocks(historical_initial.state);
		const historical_work = historical_blocks.find((block) => block.type === "work_group");
		if (historical_work?.type !== "work_group") throw new Error("work must render");
		expect(historical_work.details.map((item) => item.id)).toEqual([
			"unspecified-progress",
			"legacy-activity",
		]);
		expect(
			historical_blocks.filter(
				(block) => block.type === "item" && block.item.type === "assistant_message",
			),
		).toEqual([
			expect.objectContaining({ item: expect.objectContaining({ id: "unspecified-final" }) }),
		]);

		const active = Schema.decodeUnknownSync(ConversationSnapshot)({
			...legacy_progress,
			conversation_id: "conversation-active-unspecified-progress",
			items: legacy_progress.items.map((item) =>
				item.type === "work_session"
					? {
							...item,
							ended_at: undefined,
							lifecycle: "active",
							status: "active",
						}
					: item,
			),
			turns: legacy_progress.turns.map((turn) => ({
				...turn,
				lifecycle: "active",
			})),
		});
		const active_initial = MakeConversationViewState(active);
		if (active_initial._tag !== "applied") throw new Error("fixture must initialize");
		const active_blocks = MakeConversationRenderBlocks(active_initial.state);
		const active_work = active_blocks.find((block) => block.type === "work_group");
		if (active_work?.type !== "work_group") throw new Error("work must render");
		expect(active_work.details.map((item) => item.id)).toEqual([
			"unspecified-progress",
			"legacy-activity",
			"unspecified-final",
		]);
		expect(
			active_blocks.some(
				(block) => block.type === "item" && block.item.type === "assistant_message",
			),
		).toBe(false);
	});

	it("promotes a settled reply once its own lifecycle completes, even if the turn's completion patch lags", () => {
		// Mirrors Claude: phase is always "unspecified" and the turn's own
		// completion patch can lag or drop behind a stuck reasoning item fixed
		// elsewhere. The reply's own lifecycle already says "completed" and it
		// is the last item in the turn, so it must not stay hidden forever.
		const settled_at = "2026-07-29T12:00:02.000Z";
		const dangling_turn = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-dangling-turn",
			items: [
				{
					created_at: "2026-07-29T12:00:00.000Z",
					ended_at: settled_at,
					id: "work-dangling",
					lifecycle: "completed",
					ordinal: 1,
					references: [],
					revision: 1,
					source_refs: [],
					started_at: "2026-07-29T12:00:00.000Z",
					status: "completed",
					title: "Agent work",
					turn_id: "turn-dangling",
					type: "work_session",
					updated_at: settled_at,
				},
				{
					created_at: "2026-07-29T12:00:01.000Z",
					id: "reply-dangling",
					lifecycle: "completed",
					ordinal: 2,
					phase: "unspecified",
					references: [],
					revision: 0,
					source_refs: [],
					text: "Here is the answer.",
					turn_id: "turn-dangling",
					type: "assistant_message",
					updated_at: settled_at,
				},
			],
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-dangling-turn",
			turns: [
				{
					created_at: "2026-07-29T12:00:00.000Z",
					id: "turn-dangling",
					// The turn's own completion patch never arrived (dropped/delayed).
					lifecycle: "streaming",
					ordinal: 0,
					references: [],
					revision: 1,
					source_refs: [],
					type: "turn",
					updated_at: "2026-07-29T12:00:00.000Z",
				},
			],
			updated_at: settled_at,
		});
		const initial = MakeConversationViewState(dangling_turn);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const work = blocks.find((block) => block.type === "work_group");
		if (work?.type !== "work_group") throw new Error("work must render");

		expect(work.details.some((item) => item.id === "reply-dangling")).toBe(false);
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "reply-dangling"),
		).toBe(true);
	});

	it("does not promote a completed message early when a later item follows it in the same turn", () => {
		const settled_at = "2026-07-29T12:00:03.000Z";
		const mid_turn_tool_call = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-mid-turn-tool-call",
			items: [
				{
					created_at: "2026-07-29T12:00:00.000Z",
					ended_at: settled_at,
					id: "work-mid-turn",
					lifecycle: "completed",
					ordinal: 1,
					references: [],
					revision: 1,
					source_refs: [],
					started_at: "2026-07-29T12:00:00.000Z",
					status: "completed",
					title: "Agent work",
					turn_id: "turn-mid-turn",
					type: "work_session",
					updated_at: settled_at,
				},
				{
					created_at: "2026-07-29T12:00:01.000Z",
					id: "interim-reply",
					lifecycle: "completed",
					ordinal: 2,
					phase: "unspecified",
					references: [],
					revision: 0,
					source_refs: [],
					text: "Let me check something first.",
					turn_id: "turn-mid-turn",
					type: "assistant_message",
					updated_at: "2026-07-29T12:00:01.000Z",
				},
				{
					created_at: "2026-07-29T12:00:02.000Z",
					id: "later-activity",
					kind: "terminal_activity",
					label: "Ran a command",
					lifecycle: "completed",
					ordinal: 3,
					references: [],
					revision: 0,
					source_refs: [],
					status: "completed",
					turn_id: "turn-mid-turn",
					type: "activity",
					updated_at: settled_at,
				},
			],
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-mid-turn-tool-call",
			turns: [
				{
					created_at: "2026-07-29T12:00:00.000Z",
					id: "turn-mid-turn",
					lifecycle: "streaming",
					ordinal: 0,
					references: [],
					revision: 1,
					source_refs: [],
					type: "turn",
					updated_at: "2026-07-29T12:00:00.000Z",
				},
			],
			updated_at: settled_at,
		});
		const initial = MakeConversationViewState(mid_turn_tool_call);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const work = blocks.find((block) => block.type === "work_group");
		if (work?.type !== "work_group") throw new Error("work must render");

		expect(work.details.some((item) => item.id === "interim-reply")).toBe(true);
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "interim-reply"),
		).toBe(false);
	});

	it("places one changed-files card after the turn's final response", () => {
		const initial = MakeConversationViewState(MakeMockConversation("changes-test"));
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const assistant_index = blocks.findIndex(
			(block) => block.type === "item" && block.item.id === "mock-assistant-2",
		);
		const changes_index = blocks.findIndex(
			(block) => block.type === "changes" && block.turn_id === "mock-turn-2",
		);
		const changes = blocks[changes_index];
		const work = blocks.find(
			(block) => block.type === "work_group" && block.session.id === "mock-work-2",
		);

		expect(changes_index).toBe(assistant_index + 1);
		expect(changes).toMatchObject({
			id: "changes:mock-turn-2",
			type: "changes",
			change_sets: [{ id: "mock-change-set-2" }],
		});
		if (changes?.type === "changes") expect(changes.files).toHaveLength(4);
		if (work?.type === "work_group") {
			expect(
				work.details.every(
					(item) => item.type !== "change_set" && item.type !== "file_change",
				),
			).toBe(true);
		}
	});

	it("places one settled-turn footer after changes for a completed final response", () => {
		const initial = MakeConversationViewState(MakeMockConversation("footer-test"));
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const changes_index = blocks.findIndex(
			(block) => block.type === "changes" && block.turn_id === "mock-turn-2",
		);
		const footer_index = blocks.findIndex(
			(block) => block.type === "turn_footer" && block.turn_id === "mock-turn-2",
		);

		expect(footer_index).toBe(changes_index + 1);
		expect(blocks[footer_index]).toMatchObject({
			id: "footer:mock-turn-2",
			text: expect.any(String),
			type: "turn_footer",
		});
		expect(
			blocks.some((block) => block.type === "turn_footer" && block.turn_id === "mock-turn-5"),
		).toBe(false);
	});

	it("coalesces legacy provider and Artisan work sessions for the same run", () => {
		const completed_at = "2026-07-26T01:38:36.000Z";
		const legacy = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-legacy-run",
			items: [
				{
					created_at: "2026-07-26T01:38:30.000Z",
					id: "message:user-run-1",
					lifecycle: "completed",
					ordinal: 3,
					references: [],
					revision: 0,
					run_id: "run-1",
					source_refs: [],
					text: "Hello",
					turn_id: "turn:user-run-1",
					type: "user_message",
					updated_at: "2026-07-26T01:38:30.000Z",
				},
				{
					created_at: "2026-07-26T01:38:31.000Z",
					ended_at: completed_at,
					id: "work:run:run-1",
					lifecycle: "completed",
					ordinal: 4,
					references: [],
					revision: 1,
					run_id: "run-1",
					source_refs: [],
					started_at: "2026-07-26T01:38:31.000Z",
					status: "completed",
					title: "Agent work",
					turn_id: "run:run-1",
					type: "work_session",
					updated_at: completed_at,
				},
				{
					created_at: "2026-07-26T01:38:32.000Z",
					ended_at: "2026-07-26T01:38:35.000Z",
					id: "work:exec:run-1:turn",
					lifecycle: "completed",
					ordinal: 5,
					references: [],
					revision: 1,
					run_id: "run-1",
					source_refs: [],
					started_at: "2026-07-26T01:38:32.000Z",
					status: "completed",
					title: "Agent work",
					turn_id: "exec:run-1:turn",
					type: "work_session",
					updated_at: "2026-07-26T01:38:35.000Z",
				},
				{
					created_at: completed_at,
					id: "message:run-1",
					lifecycle: "completed",
					ordinal: 6,
					phase: "final",
					references: [],
					revision: 0,
					run_id: "run-1",
					source_refs: [],
					text: "Hello from Codex",
					turn_id: "exec:run-1:turn",
					type: "assistant_message",
					updated_at: completed_at,
				},
			],
			journal_sequence: 1,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-legacy-run",
			turns: [
				{
					created_at: "2026-07-26T01:38:31.000Z",
					id: "run:run-1",
					lifecycle: "completed",
					ordinal: 0,
					references: [],
					revision: 1,
					run_id: "run-1",
					source_refs: [],
					type: "turn",
					updated_at: completed_at,
				},
				{
					created_at: "2026-07-26T01:38:32.000Z",
					id: "exec:run-1:turn",
					lifecycle: "completed",
					ordinal: 1,
					references: [],
					revision: 1,
					run_id: "run-1",
					source_refs: [],
					type: "turn",
					updated_at: completed_at,
				},
				{
					created_at: "2026-07-26T01:38:30.000Z",
					id: "turn:user-run-1",
					lifecycle: "completed",
					ordinal: 2,
					references: [],
					revision: 0,
					run_id: "run-1",
					source_refs: [],
					type: "turn",
					updated_at: "2026-07-26T01:38:30.000Z",
				},
			],
			updated_at: completed_at,
		});
		const initial = MakeConversationViewState(legacy);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const work = blocks.filter((block) => block.type === "work_group");

		expect(work).toHaveLength(1);
		expect(work[0]).toMatchObject({
			duration_kind: "thought",
			details: [],
			session: {
				id: "work:run:run-1",
				started_at: "2026-07-26T01:38:31.000Z",
			},
		});
		expect(
			blocks.some(
				(block) => block.type === "item" && block.item.id === "work:exec:run-1:turn",
			),
		).toBe(false);
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "message:user-run-1"),
		).toBe(true);
		expect(blocks.at(-1)).toMatchObject({
			id: "footer:run:run-1",
			text: "Hello from Codex",
			turn_id: "run:run-1",
			type: "turn_footer",
		});
		expect(
			blocks
				.filter(
					(block) =>
						block.type === "work_group" ||
						(block.type === "item" && block.item.id === "message:run-1") ||
						block.type === "turn_footer",
				)
				.map((block) => block.turn_id),
		).toEqual(["run:run-1", "run:run-1", "run:run-1"]);
		const final_index = blocks.findIndex(
			(block) => block.type === "item" && block.item.id === "message:run-1",
		);
		const footer_index = blocks.findIndex((block) => block.type === "turn_footer");
		expect(footer_index).toBe(final_index + 1);
	});
});
