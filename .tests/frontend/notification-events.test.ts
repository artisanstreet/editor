import type { EventEnvelope } from "@artisan/protocol";
import { describe, expect, it } from "vitest";

import {
	IsNotifiableEvent,
	SystemNotificationDecisionFor,
	type SystemNotificationContext,
} from "../../modules/frontend/src/lib/notifications/events";

const envelope = (
	payload: EventEnvelope["payload"],
	overrides: Partial<Pick<EventEnvelope, "run_id" | "thread_id">> = {},
): EventEnvelope =>
	({
		causation_id: "cause_1",
		correlation_id: "correlation_1",
		journal_sequence: 1,
		kind: "event",
		payload,
		run_id: "run_1",
		sequence: 1,
		stream_id: "stream_1",
		thread_id: "thread_1",
		...overrides,
	}) as EventEnvelope;

const context = (
	overrides: Partial<SystemNotificationContext> = {},
): SystemNotificationContext => ({
	active_thread_id: undefined,
	focused: false,
	route_path: "/t/project_1/thread_1",
	thread_title: "Rename the installer shortcut",
	...overrides,
});

const run_completed = {
	state: "completed",
	type: "run.lifecycle",
	working_directory: "/w",
} as const;

describe("SystemNotificationDecisionFor", () => {
	it("announces a completed run against the thread it belongs to", () => {
		const decision = SystemNotificationDecisionFor(envelope(run_completed), context());

		expect(decision).toEqual({
			_tag: "show",
			notification: {
				body: "Finished.",
				category: "run_completed",
				id: "run:run_1",
				route_path: "/t/project_1/thread_1",
				title: "Rename the installer shortcut",
			},
		});
	});

	it("stays quiet for outcomes the reader caused or is still watching", () => {
		for (const state of [
			"queued",
			"running",
			"waiting",
			"interrupted",
			"cancelled",
			"closed",
		]) {
			expect(
				SystemNotificationDecisionFor(
					envelope({ state, type: "run.lifecycle", working_directory: "/w" } as never),
					context(),
				),
			).toEqual({ _tag: "ignore" });
		}
	});

	it("suppresses only the thread the reader is already looking at", () => {
		const focused_here = context({ active_thread_id: "thread_1", focused: true });
		const focused_elsewhere = context({ active_thread_id: "thread_2", focused: true });
		const unfocused = context({ active_thread_id: "thread_1", focused: false });

		expect(SystemNotificationDecisionFor(envelope(run_completed), focused_here)).toEqual({
			_tag: "ignore",
		});
		expect(SystemNotificationDecisionFor(envelope(run_completed), focused_elsewhere)._tag).toBe(
			"show",
		);
		expect(SystemNotificationDecisionFor(envelope(run_completed), unfocused)._tag).toBe("show");
	});

	it("posts a requested approval and revokes it under the same identity", () => {
		const requested = envelope({
			approval_id: "approval_1",
			description: "Run `cargo test --workspace`",
			state: "requested",
			type: "interaction.approval",
		});
		const resolved = envelope({
			approval_id: "approval_1",
			approved: true,
			description: "Run `cargo test --workspace`",
			state: "resolved",
			type: "interaction.approval",
		});

		expect(SystemNotificationDecisionFor(requested, context())).toEqual({
			_tag: "show",
			notification: {
				body: "Needs approval — Run `cargo test --workspace`",
				category: "approval",
				id: "approval:approval_1",
				route_path: "/t/project_1/thread_1",
				title: "Rename the installer shortcut",
			},
		});
		expect(SystemNotificationDecisionFor(resolved, context())).toEqual({
			_tag: "revoke",
			id: "approval:approval_1",
		});
	});

	it("revokes a resolved interaction even from the focused thread", () => {
		const resolved = envelope({
			question_id: "question_1",
			state: "resolved",
			text: "Which database should this point at?",
			type: "interaction.question",
		});

		expect(
			SystemNotificationDecisionFor(
				resolved,
				context({ active_thread_id: "thread_1", focused: true }),
			),
		).toEqual({ _tag: "revoke", id: "question:question_1" });
	});

	it("collapses and truncates a long question into one line", () => {
		const decision = SystemNotificationDecisionFor(
			envelope({
				question_id: "question_1",
				state: "requested",
				text: `Which\n  database\tshould ${"this point at".repeat(20)}?`,
				type: "interaction.question",
			}),
			context(),
		);

		expect(decision._tag).toBe("show");
		if (decision._tag !== "show") return;
		expect(decision.notification.body).toMatch(/^Has a question — Which database should this/u);
		expect(decision.notification.body).not.toContain("\n");
		expect(decision.notification.body.endsWith("…")).toBe(true);
	});

	it("names an unknown thread without inventing a route into it", () => {
		const decision = SystemNotificationDecisionFor(
			envelope(run_completed),
			context({ route_path: "/", thread_title: undefined }),
		);

		expect(decision).toMatchObject({
			_tag: "show",
			notification: { route_path: "/", title: "Artisan" },
		});
	});

	it("falls back to the thread when a run carries no identity of its own", () => {
		expect(
			SystemNotificationDecisionFor(
				envelope(run_completed, { run_id: undefined }),
				context(),
			),
		).toMatchObject({ notification: { id: "run:thread_1" } });
	});

	it("admits only the payloads a notification can be built from", () => {
		expect(IsNotifiableEvent(envelope(run_completed))).toBe(true);
		expect(
			IsNotifiableEvent(
				envelope({
					assumption: "The installer owns the shortcut",
					message_id: "message_1",
					type: "intake.assumption_recorded",
				}),
			),
		).toBe(false);
	});
});
