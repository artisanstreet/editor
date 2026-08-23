import { describe, expect, it } from "vitest";

import {
	work_session_disclosure,
	work_session_initially_open,
} from "../../modules/frontend/src/lib/conversation/presentation";

describe("conversation presentation", () => {
	it("keeps settled work history collapsed when it mounts with details or failure", () => {
		expect(
			work_session_initially_open({
				has_details: true,
				unsuccessful: false,
				working: false,
			}),
		).toBe(false);
		expect(
			work_session_initially_open({
				has_details: false,
				unsuccessful: false,
				working: false,
			}),
		).toBe(false);
		expect(
			work_session_initially_open({
				has_details: false,
				unsuccessful: true,
				working: false,
			}),
		).toBe(false);
		expect(
			work_session_initially_open({
				has_details: false,
				unsuccessful: false,
				working: true,
			}),
		).toBe(true);
	});

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
});
