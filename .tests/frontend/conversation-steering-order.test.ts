import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ConversationPatch, ConversationSnapshot } from "@artisan/protocol";
import {
	ApplyConversationViewPatch,
	MakeConversationRenderBlocks,
	MakeConversationRenderWindow,
	MakeConversationViewState,
} from "../../modules/frontend/src/lib/conversation/store";

const DecodePatch = (value: unknown) => Schema.decodeUnknownSync(ConversationPatch)(value);

describe("conversation steering order", () => {
	it("keeps same-item post-steer text below the user through append and completion", () => {
		const snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			conversation_id: "conversation-steering-order",
			items: [
				{
					created_at: "2026-08-10T20:00:00.000Z",
					id: "work:run:active",
					lifecycle: "active",
					ordinal: 1,
					references: [],
					revision: 0,
					run_id: "active",
					source_refs: [],
					started_at: "2026-08-10T20:00:00.000Z",
					status: "active",
					title: "Agent work",
					turn_id: "run:active",
					type: "work_session",
					updated_at: "2026-08-10T20:00:00.000Z",
				},
				{
					created_at: "2026-08-10T20:00:01.000Z",
					id: "activity:before-steer",
					kind: "terminal_activity",
					label: "Ran verification",
					lifecycle: "completed",
					ordinal: 2,
					references: [],
					revision: 0,
					run_id: "active",
					source_refs: [],
					status: "completed",
					turn_id: "run:active",
					type: "activity",
					updated_at: "2026-08-10T20:00:01.000Z",
				},
				{
					created_at: "2026-08-10T20:00:02.000Z",
					id: "assistant:straddled-steer",
					lifecycle: "completed",
					ordinal: 3,
					phase: "unspecified",
					references: [],
					revision: 3,
					run_id: "active",
					source_refs: [],
					steering_fragment_boundaries: [
						{ after_item_id: "message:steer", text_offset: 11 },
					],
					text: "Prior work. New response",
					turn_id: "run:active",
					type: "assistant_message",
					updated_at: "2026-08-10T20:00:05.000Z",
				},
				{
					created_at: "2026-08-10T20:00:03.000Z",
					id: "message:steer",
					lifecycle: "completed",
					ordinal: 5,
					references: [],
					revision: 0,
					run_id: "active",
					source_refs: [],
					text: "Use the curated data set",
					turn_id: "turn:user:steer",
					type: "user_message",
					updated_at: "2026-08-10T20:00:03.000Z",
				},
			],
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id: "thread-steering-order",
			turns: [
				{
					created_at: "2026-08-10T20:00:00.000Z",
					id: "run:active",
					lifecycle: "active",
					ordinal: 0,
					references: [],
					revision: 0,
					run_id: "active",
					source_refs: [],
					type: "turn",
					updated_at: "2026-08-10T20:00:03.000Z",
				},
				{
					created_at: "2026-08-10T20:00:02.000Z",
					id: "turn:user:steer",
					lifecycle: "completed",
					ordinal: 4,
					references: [],
					revision: 0,
					run_id: "active",
					source_refs: [],
					type: "turn",
					updated_at: "2026-08-10T20:00:02.000Z",
				},
			],
			updated_at: "2026-08-10T20:00:05.000Z",
		});
		const state = MakeConversationViewState(snapshot);
		if (state._tag !== "applied") throw new Error("fixture must initialize");

		const ExpectSteeringOrder = (blocks: ReturnType<typeof MakeConversationRenderBlocks>) => {
			const work = blocks.find((block) => block.type === "work_group");
			if (work?.type !== "work_group") throw new Error("work group must render");
			expect(
				blocks
					.filter((block) => block.type === "work_group" || block.type === "item")
					.map((block) =>
						block.type === "work_group" ? block.session.id : block.item.id,
					),
			).toEqual([
				"work:run:active",
				"message:steer",
				JSON.stringify(["assistant-fragment", "assistant:straddled-steer", 11]),
			]);
			expect(work.details.map((item) => item.id)).toEqual([
				"activity:before-steer",
				JSON.stringify(["assistant-fragment", "assistant:straddled-steer", 0]),
			]);
			const suffix = blocks.find(
				(block) =>
					block.type === "item" &&
					block.item.id ===
						JSON.stringify(["assistant-fragment", "assistant:straddled-steer", 11]),
			);
			expect(suffix).toMatchObject({
				item: { lifecycle: "completed", text: " New response" },
			});
		};

		ExpectSteeringOrder(MakeConversationRenderBlocks(state.state));

		const initial_snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			...snapshot,
			items: snapshot.items.slice(0, 3).map((item) =>
				item.id === "assistant:straddled-steer"
					? {
							...item,
							lifecycle: "streaming" as const,
							revision: 0,
							steering_fragment_boundaries: undefined,
							text: "Prior work.",
						}
					: item,
			),
			turns: snapshot.turns.slice(0, 1),
			updated_at: "2026-08-10T20:00:02.000Z",
		});
		const initial = MakeConversationViewState(initial_snapshot);
		if (initial._tag !== "applied") throw new Error("initial fixture must initialize");
		const with_user_turn = ApplyConversationViewPatch(
			initial.state,
			DecodePatch({
				patch_id: "steering-user-turn",
				sequence: 1,
				turn: snapshot.turns[1],
				type: "turn_upsert",
			}),
		);
		if (with_user_turn._tag !== "applied") throw new Error("user turn must apply");
		const with_user_message = ApplyConversationViewPatch(
			with_user_turn.state,
			DecodePatch({
				item: snapshot.items[3],
				patch_id: "steering-user-message",
				sequence: 2,
				type: "item_upsert",
			}),
		);
		if (with_user_message._tag !== "applied") throw new Error("user message must apply");
		const with_steering_boundary = ApplyConversationViewPatch(
			with_user_message.state,
			DecodePatch({
				item: {
					...snapshot.items[2],
					lifecycle: "streaming",
					revision: 1,
					text: "Prior work.",
				},
				patch_id: "steering-boundary",
				sequence: 3,
				type: "item_upsert",
			}),
		);
		if (with_steering_boundary._tag !== "applied") throw new Error("boundary must apply");
		expect(
			MakeConversationRenderWindow(with_steering_boundary.state, 24).blocks.filter(
				(block) => block.type === "item" && block.item.type === "assistant_message",
			),
		).toHaveLength(0);
		const with_append = ApplyConversationViewPatch(
			with_steering_boundary.state,
			DecodePatch({
				item_id: "assistant:straddled-steer",
				patch_id: "steering-suffix-append",
				revision: 2,
				sequence: 4,
				text: " New response",
				type: "item_append",
			}),
		);
		if (with_append._tag !== "applied") throw new Error("suffix must append");
		const completed = ApplyConversationViewPatch(
			with_append.state,
			DecodePatch({
				item_id: "assistant:straddled-steer",
				lifecycle: "completed",
				patch_id: "steering-suffix-complete",
				revision: 3,
				sequence: 5,
				type: "item_lifecycle",
			}),
		);
		if (completed._tag !== "applied") throw new Error("suffix must complete");
		ExpectSteeringOrder(MakeConversationRenderWindow(completed.state, 24).blocks);

		const fresh_snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			...snapshot,
			items: [
				snapshot.items[0],
				snapshot.items[1],
				snapshot.items[3],
				{
					...snapshot.items[2],
					id: "assistant:after-steer",
					lifecycle: "streaming",
					ordinal: 6,
					revision: 0,
					steering_fragment_boundaries: undefined,
					text: "Fresh response",
				},
			],
		});
		const fresh = MakeConversationViewState(fresh_snapshot);
		if (fresh._tag !== "applied") throw new Error("fresh response fixture must initialize");
		expect(
			MakeConversationRenderBlocks(fresh.state)
				.filter((block) => block.type === "work_group" || block.type === "item")
				.map((block) => (block.type === "work_group" ? block.session.id : block.item.id)),
		).toEqual(["work:run:active", "message:steer", "assistant:after-steer"]);

		const continued_work_snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			...snapshot,
			items: [
				snapshot.items[0],
				{
					created_at: "2026-08-10T20:00:01.000Z",
					id: "reasoning:straddled-steer",
					lifecycle: "streaming",
					ordinal: 2,
					references: [],
					revision: 2,
					run_id: "active",
					source_refs: [],
					steering_fragment_boundaries: [
						{ after_item_id: "message:steer", text_offset: 12 },
					],
					text: "Before steer. After steer reasoning",
					turn_id: "run:active",
					type: "reasoning_summary",
					updated_at: "2026-08-10T20:00:05.000Z",
				},
				snapshot.items[3],
				{
					...snapshot.items[1],
					id: "activity:after-steer",
					ordinal: 6,
					updated_at: "2026-08-10T20:00:06.000Z",
				},
			],
		});
		const continued_work = MakeConversationViewState(continued_work_snapshot);
		if (continued_work._tag !== "applied") throw new Error("continued work must initialize");
		const continued_blocks = MakeConversationRenderBlocks(continued_work.state);
		expect(
			continued_blocks
				.filter((block) => block.type === "work_group" || block.type === "item")
				.map((block) => (block.type === "work_group" ? block.session.id : block.item.id)),
		).toEqual([
			"work:run:active",
			"message:steer",
			JSON.stringify(["assistant-fragment", "reasoning:straddled-steer", 12]),
			"activity:after-steer",
		]);
		const continued_work_group = continued_blocks.find((block) => block.type === "work_group");
		expect(continued_work_group).toMatchObject({
			details: [expect.objectContaining({ text: "Before steer" })],
		});

		const two_steer_snapshot = Schema.decodeUnknownSync(ConversationSnapshot)({
			...snapshot,
			items: [
				snapshot.items[0],
				snapshot.items[1],
				{
					...snapshot.items[2],
					lifecycle: "streaming",
					revision: 2,
					steering_fragment_boundaries: [
						{ after_item_id: "message:steer", text_offset: 6 },
						{ after_item_id: "message:steer-b", text_offset: 12 },
					],
					text: "prefixmiddletail",
				},
				snapshot.items[3],
				{
					...snapshot.items[3],
					created_at: "2026-08-10T20:00:04.000Z",
					id: "message:steer-b",
					ordinal: 7,
					text: "Second steer",
					turn_id: "turn:user:steer-b",
					updated_at: "2026-08-10T20:00:04.000Z",
				},
			],
			turns: [
				snapshot.turns[0],
				snapshot.turns[1],
				{
					...snapshot.turns[1],
					created_at: "2026-08-10T20:00:04.000Z",
					id: "turn:user:steer-b",
					ordinal: 6,
					updated_at: "2026-08-10T20:00:04.000Z",
				},
			],
		});
		const two_steer = MakeConversationViewState(two_steer_snapshot);
		if (two_steer._tag !== "applied") throw new Error("two-steer fixture must initialize");
		const FragmentLifecycles = (blocks: ReturnType<typeof MakeConversationRenderBlocks>) =>
			blocks
				.flatMap((block) =>
					block.type === "work_group"
						? block.details
						: block.type === "item"
							? [block.item]
							: [],
				)
				.filter(
					(item) =>
						item.type === "assistant_message" &&
						item.id.startsWith('["assistant-fragment","assistant:straddled-steer",'),
				);
		const VisibleIds = (blocks: ReturnType<typeof MakeConversationRenderBlocks>) =>
			blocks
				.filter((block) => block.type === "work_group" || block.type === "item")
				.map((block) => (block.type === "work_group" ? block.session.id : block.item.id));
		const two_steer_blocks = MakeConversationRenderBlocks(two_steer.state);
		expect(VisibleIds(two_steer_blocks)).toEqual([
			"work:run:active",
			"message:steer",
			JSON.stringify(["assistant-fragment", "assistant:straddled-steer", 6]),
			"message:steer-b",
			JSON.stringify(["assistant-fragment", "assistant:straddled-steer", 12]),
		]);
		expect(FragmentLifecycles(two_steer_blocks)).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ lifecycle: "completed", text: "prefix" }),
				expect.objectContaining({ lifecycle: "completed", text: "middle" }),
				expect.objectContaining({ lifecycle: "streaming", text: "tail" }),
			]),
		);
		const two_steer_completed = ApplyConversationViewPatch(
			two_steer.state,
			DecodePatch({
				item_id: "assistant:straddled-steer",
				lifecycle: "completed",
				patch_id: "two-steer-tail-complete",
				revision: 3,
				sequence: 1,
				type: "item_lifecycle",
			}),
		);
		if (two_steer_completed._tag !== "applied") throw new Error("tail must complete");
		const completed_two_steer_blocks = MakeConversationRenderBlocks(two_steer_completed.state);
		expect(VisibleIds(completed_two_steer_blocks)).toEqual([
			"work:run:active",
			"message:steer",
			JSON.stringify(["assistant-fragment", "assistant:straddled-steer", 6]),
			"message:steer-b",
			JSON.stringify(["assistant-fragment", "assistant:straddled-steer", 12]),
		]);
		expect(FragmentLifecycles(completed_two_steer_blocks)).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ lifecycle: "completed", text: "prefix" }),
				expect.objectContaining({ lifecycle: "completed", text: "middle" }),
				expect.objectContaining({ lifecycle: "completed", text: "tail" }),
			]),
		);
	});
});
