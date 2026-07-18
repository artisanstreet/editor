import { describe, expect, it } from "@effect/vitest";
import { Effect, Option, Stream, SubscriptionRef } from "effect";

import {
	ApplyThreadListUpdate,
	IsCurrentThreadSelection,
	ShouldRefreshForConnection,
	ToLiveWorkspacePhase,
	type LiveWorkspaceSnapshot,
} from "../../modules/frontend/src/lib/live-workspace/store";

const EmptySnapshot: LiveWorkspaceSnapshot = {
	error: Option.none(),
	global_guidance: Option.none(),
	model_behaviour: Option.none(),
	phase: "ready",
	selected_thread_id: Option.some("thread-1"),
	thread_work: Option.none(),
	threads: [],
};

describe("live workspace state", () => {
	it("maps the desktop lifecycle into explicit renderer states", () => {
		expect(ToLiveWorkspacePhase("connecting")).toBe("connecting");
		expect(ToLiveWorkspacePhase("ready")).toBe("ready");
		expect(ToLiveWorkspacePhase("reconnecting")).toBe("reconnecting");
		expect(ToLiveWorkspacePhase("stale")).toBe("stale");
		expect(ToLiveWorkspacePhase("error")).toBe("error");
		expect(ToLiveWorkspacePhase("unavailable")).toBe("error");
	});

	it("reloads projections after a reconnect becomes ready", () => {
		expect(ShouldRefreshForConnection("reconnecting")).toBe(false);
		expect(ShouldRefreshForConnection("ready")).toBe(true);
	});

	it("clears a removed selected thread rather than retaining stale local state", () => {
		const updated = ApplyThreadListUpdate(EmptySnapshot, {
			thread_id: "thread-1",
			type: "remove",
		});

		expect(Option.isNone(updated.selected_thread_id)).toBe(true);
		expect(updated.threads).toEqual([]);
	});

	it("clears selected work when an authoritative snapshot no longer contains it", () => {
		const updated = ApplyThreadListUpdate(EmptySnapshot, {
			threads: [],
			type: "snapshot",
		});

		expect(updated.threads).toEqual([]);
		expect(Option.isNone(updated.selected_thread_id)).toBe(true);
		expect(Option.isNone(updated.thread_work)).toBe(true);
	});

	it("rejects a late work result after the user selected another thread", () => {
		const selected_other_thread = {
			...EmptySnapshot,
			selected_thread_id: Option.some("thread-2"),
		};

		expect(
			IsCurrentThreadSelection(selected_other_thread, 2, "thread-1", 1),
		).toBe(false);
	});

	it.effect("replays current state to a late renderer subscription", () =>
		Effect.gen(function* () {
			const state = yield* SubscriptionRef.make("before-subscribe");
			const replayed = yield* SubscriptionRef.changes(state).pipe(
				Stream.take(1),
				Stream.runHead,
			);

			expect(Option.getOrUndefined(replayed)).toBe("before-subscribe");
		}),
	);
});
