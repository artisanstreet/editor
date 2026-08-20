import { readFileSync } from "node:fs";

import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ConversationSnapshot } from "@artisan/protocol";
import {
	MakeConversationRenderBlocks,
	MakeConversationViewState,
} from "../../modules/frontend/src/lib/conversation/store";
import { ConversationSteeringAcknowledged } from "../../modules/frontend/src/lib/conversation/steering";

const steer_command_id = "command-steer";

const MakeSnapshot = (input: { readonly post_steer_activity: boolean }) =>
	Schema.decodeUnknownSync(ConversationSnapshot)({
		conversation_id: "conversation-steering-settlement",
		items: [
			{
				created_at: "2026-08-15T09:00:00.000Z",
				id: "work:run:active",
				lifecycle: "active",
				ordinal: 1,
				references: [],
				revision: 0,
				run_id: "active",
				source_refs: [],
				started_at: "2026-08-15T09:00:00.000Z",
				status: "active",
				title: "Agent work",
				turn_id: "run:active",
				type: "work_session",
				updated_at: "2026-08-15T09:00:00.000Z",
			},
			{
				created_at: "2026-08-15T09:00:01.000Z",
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
				updated_at: "2026-08-15T09:00:01.000Z",
			},
			{
				created_at: "2026-08-15T09:00:02.000Z",
				id: "message:steer",
				lifecycle: "completed",
				ordinal: 4,
				references: [],
				revision: 0,
				run_id: "active",
				source_refs: [{ provider: "artisan", reference: steer_command_id }],
				text: "Use the curated data set",
				turn_id: "turn:user:steer",
				type: "user_message",
				updated_at: "2026-08-15T09:00:02.000Z",
			},
			...(input.post_steer_activity
				? [
						{
							created_at: "2026-08-15T09:00:03.000Z",
							id: "activity:after-steer",
							kind: "terminal_activity",
							label: "Read the curated data set",
							lifecycle: "completed",
							ordinal: 5,
							references: [],
							revision: 0,
							run_id: "active",
							source_refs: [],
							status: "completed",
							turn_id: "run:active",
							type: "activity",
							updated_at: "2026-08-15T09:00:03.000Z",
						},
					]
				: []),
		],
		journal_sequence: 0,
		last_patch_sequence: 0,
		schema_version: 1,
		thread_id: "thread-steering-settlement",
		turns: [
			{
				created_at: "2026-08-15T09:00:00.000Z",
				id: "run:active",
				lifecycle: "active",
				ordinal: 0,
				references: [],
				revision: 0,
				run_id: "active",
				source_refs: [],
				type: "turn",
				updated_at: "2026-08-15T09:00:03.000Z",
			},
			{
				created_at: "2026-08-15T09:00:02.000Z",
				id: "turn:user:steer",
				lifecycle: "completed",
				ordinal: 3,
				references: [],
				revision: 0,
				run_id: "active",
				source_refs: [],
				type: "turn",
				updated_at: "2026-08-15T09:00:02.000Z",
			},
		],
		updated_at: "2026-08-15T09:00:03.000Z",
	});

describe("steering settlement", () => {
	/**
	 * Settling on Artisan's own echo of the steering message made the "Steering"
	 * label live for however long a local Forge needs to project a user turn —
	 * milliseconds, and below perceptibility.
	 */
	it("is unacknowledged while only the user's own message has landed", () => {
		const snapshot = MakeSnapshot({ post_steer_activity: false });

		expect(
			ConversationSteeringAcknowledged(snapshot.items, snapshot.turns, steer_command_id),
		).toBe(false);
	});

	it("is acknowledged once the run projects work after the steer", () => {
		const snapshot = MakeSnapshot({ post_steer_activity: true });

		expect(
			ConversationSteeringAcknowledged(snapshot.items, snapshot.turns, steer_command_id),
		).toBe(true);
	});

	/** A command that never projected a message has nothing to acknowledge yet. */
	it("is unacknowledged for a command with no canonical message", () => {
		const snapshot = MakeSnapshot({ post_steer_activity: true });

		expect(
			ConversationSteeringAcknowledged(snapshot.items, snapshot.turns, "command-other"),
		).toBe(false);
	});

	/**
	 * A steer whose run dies unsettled — orphaned by a host restart — can be
	 * taken up by a fresh run instead. The reaction is on screen below the steer
	 * either way, so waiting for the original run held "Steering" open until the
	 * projection deadline expired with the answer already visible.
	 */
	it("is acknowledged when a successor run projects work after the steer", () => {
		const snapshot = MakeSnapshot({ post_steer_activity: true });
		const items = snapshot.items.map((item) =>
			item.id === "activity:after-steer" ? { ...item, run_id: "successor" } : item,
		);

		expect(ConversationSteeringAcknowledged(items, snapshot.turns, steer_command_id)).toBe(
			true,
		);
	});

	/** A later queued user message is not an engine reaction. */
	it("stays unacknowledged when only another user message follows the steer", () => {
		const snapshot = MakeSnapshot({ post_steer_activity: true });
		const items = snapshot.items.map((item) =>
			item.id === "activity:after-steer"
				? { ...item, type: "user_message" as const, text: "And another thing" }
				: item,
		);

		expect(ConversationSteeringAcknowledged(items, snapshot.turns, steer_command_id)).toBe(
			false,
		);
	});

	/**
	 * A run that ends without ever taking up the steer still answers it. Without
	 * this the label would hang open for the life of the route scope.
	 */
	it("is acknowledged when the steering turn settles with no further work", () => {
		const snapshot = MakeSnapshot({ post_steer_activity: false });
		const turns = snapshot.turns.map((turn) =>
			turn.id === "turn:user:steer" ? turn : { ...turn, lifecycle: "failed" as const },
		);

		expect(ConversationSteeringAcknowledged(snapshot.items, turns, steer_command_id)).toBe(
			true,
		);
	});
});

describe("steered work session position", () => {
	/**
	 * The turn has one status line and it belongs at the end of the flow. Steering
	 * lifts later work out of the session and renders it below the user's
	 * message, which stranded a live "Calculating" above the steer while the work
	 * it narrated continued underneath.
	 */
	it("marks the work session superseded once post-steer work renders below it", () => {
		const state = MakeConversationViewState(MakeSnapshot({ post_steer_activity: true }));
		if (state._tag !== "applied") throw new Error("fixture must initialize");
		const blocks = MakeConversationRenderBlocks(state.state);
		const work = blocks.find((block) => block.type === "work_group");

		if (work?.type !== "work_group") throw new Error("work group must render");
		expect(work.superseded).toBe(true);
	});

	it("leaves an unsteered session owning the turn's status line", () => {
		const state = MakeConversationViewState(MakeSnapshot({ post_steer_activity: false }));
		if (state._tag !== "applied") throw new Error("fixture must initialize");
		const blocks = MakeConversationRenderBlocks(state.state);
		const work = blocks.find((block) => block.type === "work_group");

		if (work?.type !== "work_group") throw new Error("work group must render");
		expect(work.superseded).toBe(false);
	});
});

describe("steering settlement never hangs", () => {
	const route = readFileSync(
		"modules/frontend/src/routes/components/thread-route.svelte",
		"utf8",
	);
	const composer = readFileSync(
		"modules/frontend/src/routes/components/thread-composer.svelte",
		"utf8",
	);

	/**
	 * The waiter is satisfied by a conversation item appearing, which the
	 * renderer observes rather than performs. When the command that would
	 * produce it never lands, the condition can never become true — a steer
	 * whose message never reached Forge left the composer holding its pending
	 * lip forever, so the steering label stayed in the transcript with no error
	 * and no way back except restarting the app.
	 */
	it("bounds the projection wait instead of parking on evidence that may never come", () => {
		expect(route).toContain("const projection_waiter_deadline =");
		expect(route).toContain("Effect.timeoutOrElse({");
		expect(route).toContain("duration: projection_waiter_deadline");
		expect(route).toContain("Artisan never saw this reach the conversation.");
	});

	/** Expiry must release the held lip and lower the label and say why, not fail into silence. */
	it("releases the steering lip and reports when the steer is never confirmed", () => {
		expect(composer).toContain("Could not confirm the steer");
		expect(composer).toContain("Effect.ensuring(steering.Settle(steer_generation))");
		expect(composer.indexOf('ReportActionFailure("Could not confirm the steer"')).toBeLessThan(
			composer.indexOf("Effect.ensuring(steering.Settle(steer_generation))"),
		);
	});
});
