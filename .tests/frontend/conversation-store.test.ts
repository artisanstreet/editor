import { describe, expect, it } from "vitest";
import { Schema } from "effect";
import { ConversationPatch, ConversationSnapshot } from "@artisan/protocol";
import {
	ApplyConversationViewPatch,
	CanReplaceConversationSnapshot,
	MakeConversationRenderBlocks,
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

	it("appends and completes the same streaming message", () => {
		const initial = MakeConversationViewState(snapshot);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");
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
		expect(completed.state.items_by_id.get("item-a")).toMatchObject({
			text: "Hello world",
			lifecycle: "completed",
		});
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

	it("hides resolved approval receipts under worked while keeping requests visible", () => {
		const mock = MakeMockConversation("approval-grouping");
		const with_approvals = Schema.decodeUnknownSync(ConversationSnapshot)({
			...mock,
			items: [
				...mock.items,
				{
					created_at: "2026-07-24T00:00:00.000Z",
					id: "approval-resolved",
					interaction_id: "approval-response-resolved",
					lifecycle: "completed",
					ordinal: 50,
					prompt: "Run the test suite",
					references: [],
					requested_at: "2026-07-24T00:00:00.000Z",
					resolution: "Approved",
					resolved_at: "2026-07-24T00:00:01.000Z",
					revision: 1,
					source_refs: [],
					state: "approved",
					turn_id: "mock-turn-1",
					type: "approval",
					updated_at: "2026-07-24T00:00:01.000Z",
				},
				{
					created_at: "2026-07-24T00:00:00.000Z",
					id: "approval-requested",
					interaction_id: "approval-response-requested",
					lifecycle: "waiting",
					ordinal: 51,
					prompt: "Apply the generated changes",
					references: [],
					requested_at: "2026-07-24T00:00:00.000Z",
					revision: 0,
					source_refs: [],
					state: "requested",
					turn_id: "mock-turn-1",
					type: "approval",
					updated_at: "2026-07-24T00:00:00.000Z",
				},
			],
		});
		const initial = MakeConversationViewState(with_approvals);
		if (initial._tag !== "applied") throw new Error("fixture must initialize");

		const blocks = MakeConversationRenderBlocks(initial.state);
		const work = blocks.find(
			(block) => block.type === "work_group" && block.session.id === "mock-work-1",
		);
		if (work?.type !== "work_group") throw new Error("work must render");

		const detail_ids = work.details.map((item) => item.id);
		expect(detail_ids).toContain("approval-resolved");
		expect(detail_ids).not.toContain("approval-requested");
		expect(detail_ids.indexOf("mock-activity-1-1")).toBeLessThan(
			detail_ids.indexOf("approval-resolved"),
		);
		expect(
			make_conversation_trace_segments(work.details, false).some(
				(segment) => segment.type === "item" && segment.item.id === "approval-resolved",
			),
		).toBe(true);
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "approval-resolved"),
		).toBe(false);
		expect(
			blocks.some((block) => block.type === "item" && block.item.id === "approval-requested"),
		).toBe(true);

		const thought_only = Schema.decodeUnknownSync(ConversationSnapshot)({
			...with_approvals,
			items: with_approvals.items.filter(
				(item) => item.turn_id !== "mock-turn-1" || item.type !== "activity",
			),
		});
		const thought_view = MakeConversationViewState(thought_only);
		if (thought_view._tag !== "applied") throw new Error("thought fixture must initialize");
		const thought_blocks = MakeConversationRenderBlocks(thought_view.state);
		expect(
			thought_blocks.find(
				(block) => block.type === "work_group" && block.session.id === "mock-work-1",
			),
		).toMatchObject({ duration_kind: "thought" });
		expect(
			thought_blocks.some(
				(block) => block.type === "item" && block.item.id === "approval-resolved",
			),
		).toBe(true);

		const active_work = Schema.decodeUnknownSync(ConversationSnapshot)({
			...with_approvals,
			items: with_approvals.items.map((item) => {
				if (item.id !== "mock-work-1" || item.type !== "work_session") return item;
				const { ended_at: _ended_at, ...active_session } = item;
				return {
					...active_session,
					lifecycle: "active",
					status: "active",
				};
			}),
		});
		const active_view = MakeConversationViewState(active_work);
		if (active_view._tag !== "applied") throw new Error("active fixture must initialize");
		const active_blocks = MakeConversationRenderBlocks(active_view.state);
		const active_work_group = active_blocks.find(
			(block) => block.type === "work_group" && block.session.id === "mock-work-1",
		);
		if (active_work_group?.type !== "work_group") throw new Error("active work must render");
		expect(active_work_group.details).toContainEqual(
			expect.objectContaining({ id: "approval-resolved", type: "approval" }),
		);
		expect(
			active_blocks.some(
				(block) => block.type === "item" && block.item.id === "approval-resolved",
			),
		).toBe(false);
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
