import { describe, expect, it } from "vitest";

import {
	model_transition_presentation,
	work_session_disclosure,
} from "../../modules/frontend/src/lib/conversation/presentation";

describe("conversation presentation", () => {
	it("lets active work details close without unmounting the live trace", () => {
		const open = work_session_disclosure({
			details_defined: true,
			has_visible_details: true,
			open: true,
			working: true,
		});
		const closed = work_session_disclosure({
			details_defined: true,
			has_visible_details: true,
			open: false,
			working: true,
		});

		expect(open).toMatchObject({
			can_collapse: true,
			data_open: true,
			data_state: "open",
			details_mounted: true,
		});
		expect(closed).toMatchObject({
			can_collapse: true,
			data_open: false,
			data_state: "closed",
			details_hidden: true,
			details_mounted: true,
		});
	});

	it("unmounts only settled closed details", () => {
		expect(
			work_session_disclosure({
				details_defined: true,
				has_visible_details: true,
				open: false,
				working: false,
			}),
		).toMatchObject({
			can_collapse: true,
			data_open: false,
			details_hidden: true,
			details_mounted: false,
		});
	});

	it("holds a started handoff until its source model is known", () => {
		expect(model_transition_presentation("started", undefined)).toBe("pending_source");
		expect(model_transition_presentation("completed", undefined)).toBe("target_only");
		expect(model_transition_presentation("started", "old-model")).toBe("source_and_target");
	});
});
