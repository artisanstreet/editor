import { describe, expect, it } from "vitest";

import { AdvanceThreadReadTracking } from "../../modules/frontend/src/lib/root/thread-read-tracker";

const Thread = (overrides: Record<string, unknown> = {}) =>
	({
		active_run_status: undefined,
		last_activity_at: "2026-08-12T00:00:00.000Z",
		live_status: "Complete",
		reader_activity_at: "2026-08-12T00:00:00.000Z",
		thread_id: "thread-a",
		...overrides,
	}) as never;

describe("thread read departure tracking", () => {
	it("does not acknowledge entry or same-route navigation", () => {
		const entered = AdvanceThreadReadTracking(
			{},
			{
				root_visible: true,
				route_id: "a",
				thread: Thread(),
			},
		);
		expect(entered.acknowledgement).toBeUndefined();
		const same_route = AdvanceThreadReadTracking(entered.state, {
			root_visible: true,
			route_id: "a",
			thread: Thread(),
		});
		expect(same_route.acknowledgement).toBeUndefined();
	});

	it("acknowledges the observed stamp only after a real departure", () => {
		const entered = AdvanceThreadReadTracking(
			{},
			{
				root_visible: true,
				route_id: "a",
				thread: Thread(),
			},
		);
		const left = AdvanceThreadReadTracking(entered.state, {
			root_visible: false,
			route_id: "b",
			thread: Thread({ thread_id: "thread-b" }),
		});
		expect(left.acknowledgement).toEqual({
			reader_activity_at: "2026-08-12T00:00:00.000Z",
			thread_id: "thread-a",
		});
	});

	it("does not acknowledge hidden root content or an active follow-up", () => {
		const hidden = AdvanceThreadReadTracking(
			{},
			{
				root_visible: false,
				route_id: "a",
				thread: Thread(),
			},
		);
		expect(
			AdvanceThreadReadTracking(hidden.state, {
				root_visible: false,
				route_id: "b",
				thread: undefined,
			}).acknowledgement,
		).toBeUndefined();

		const active = AdvanceThreadReadTracking(
			{},
			{
				root_visible: true,
				route_id: "a",
				thread: Thread({ active_run_status: "streaming", live_status: "Working" }),
			},
		);
		expect(
			AdvanceThreadReadTracking(active.state, {
				root_visible: false,
				route_id: undefined,
				thread: undefined,
			}).acknowledgement,
		).toBeUndefined();
	});
});
