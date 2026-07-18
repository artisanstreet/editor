import { describe, expect, it } from "@effect/vitest";
import { Effect, Fiber, Option, Ref, Stream, SubscriptionRef } from "effect";
import { TestClock } from "effect/testing";

import {
	ApplyAuthoritativeThreadRefresh,
	ApplyThreadListUpdate,
	ApplyThreadListSubscriptionFailure,
	IsCurrentThreadSelection,
	RunThreadListSubscription,
	ShouldRefreshForConnection,
	ToLiveWorkspacePhase,
	type LiveWorkspaceSnapshot,
} from "../../modules/frontend/src/lib/live-workspace/store";

const EmptySnapshot: LiveWorkspaceSnapshot = {
	error: Option.none(),
	global_guidance: Option.none(),
	model_behaviour: Option.none(),
	orchestration_graph: Option.none(),
	orchestration_groups: Option.none(),
	phase: "ready",
	selected_group_id: Option.none(),
	selected_thread_id: Option.some("thread-1"),
	thread_work: Option.none(),
	transcript: Option.none(),
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

		expect(IsCurrentThreadSelection(selected_other_thread, 2, "thread-1", 1)).toBe(false);
	});

	it("clears a prior connection error after an authoritative refresh succeeds", () => {
		const recovered = ApplyAuthoritativeThreadRefresh(
			{
				...EmptySnapshot,
				error: Option.some("Temporary connection failure"),
				phase: "error",
			},
			[],
		);

		expect(Option.isNone(recovered.error)).toBe(true);
		expect(recovered.phase).toBe("empty");
	});

	it("retains the last projection but makes a lost thread subscription recoverable", () => {
		const failed = ApplyThreadListSubscriptionFailure(
			{ ...EmptySnapshot, threads: [] },
			"Thread subscription disconnected",
		);

		expect(failed.phase).toBe("error");
		expect(Option.getOrUndefined(failed.error)).toBe("Thread subscription disconnected");
	});

	it.effect(
		"retries a lost subscription and applies its recovered stream without another ready event",
		() =>
			Effect.gen(function* () {
				const attempts = yield* Ref.make(0);
				const updates = yield* Ref.make(0);
				const subscribe = Effect.gen(function* () {
					const attempt = yield* Ref.getAndUpdate(attempts, (count) => count + 1);
					if (attempt === 0) {
						return yield* Effect.fail({ message: "Initial subscription loss" });
					}

					return Stream.concat(
						Stream.fromEffect(
							Ref.update(updates, (count) => count + 1).pipe(
								Effect.as({
									journal_sequence: 1,
									threads: [],
									type: "snapshot",
								} as const),
							),
						),
						Stream.never,
					);
				});
				const fiber = yield* RunThreadListSubscription(
					subscribe,
					() => Effect.void,
					() => Effect.void,
				).pipe(Effect.forkScoped);

				yield* TestClock.adjust("100 millis");

				expect(yield* Ref.get(attempts)).toBe(2);
				expect(yield* Ref.get(updates)).toBe(1);
				yield* Fiber.interrupt(fiber);
			}).pipe(Effect.provide(TestClock.layer())),
	);

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
