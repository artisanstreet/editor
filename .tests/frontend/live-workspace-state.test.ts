import { describe, expect, it } from "@effect/vitest";
import { Option } from "effect";

import {
	ApplyThreadListUpdate,
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

	it("clears a removed selected thread rather than retaining stale local state", () => {
		const updated = ApplyThreadListUpdate(EmptySnapshot, {
			thread_id: "thread-1",
			type: "remove",
		});

		expect(Option.isNone(updated.selected_thread_id)).toBe(true);
		expect(updated.threads).toEqual([]);
	});

	it("adopts authoritative thread snapshots without manufacturing records", () => {
		const updated = ApplyThreadListUpdate(EmptySnapshot, {
			threads: [],
			type: "snapshot",
		});

		expect(updated.threads).toEqual([]);
		expect(Option.getOrUndefined(updated.selected_thread_id)).toBe("thread-1");
	});
});
