import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const workspace = readFileSync(
	"modules/frontend/src/routes/components/thread-workspace.svelte",
	"utf8",
);

describe("thread anchor layout ingress", () => {
	it("retains wake tokens with out-of-band pending geometry", () => {
		expect(workspace).toContain("Queue.unbounded<void>()");
		expect(workspace).toContain("const MarkAnchorLayoutPending");
		expect(workspace).toContain("anchor_layout_pending_smooth ||= smooth");
		expect(workspace).toContain("const smooth = anchor_layout_pending_smooth");
		expect(workspace).toContain("if (anchored_user_item_id === undefined) return;");
	});

	it("does not queue resize churn before an anchor exists", () => {
		let anchored_user_item_id: string | undefined;
		let pending = false;
		let wake_queued = false;

		const request = () => {
			if (anchored_user_item_id === undefined) return;
			pending = true;
			wake_queued = true;
		};

		request();
		expect(pending).toBe(false);
		expect(wake_queued).toBe(false);

		anchored_user_item_id = "turn-1";
		request();
		expect(pending).toBe(true);
		expect(wake_queued).toBe(true);
	});

	it("retains a smooth request when a later instant request arrives before publication", () => {
		let pending = false;
		let pending_smooth = false;
		let revision = 0;
		let published_smooth = false;
		let wake_queued = false;

		const request = (smooth: boolean) => {
			pending = true;
			pending_smooth ||= smooth;
			wake_queued ||= true;
		};
		const publish = () => {
			if (!pending) return;
			const smooth = pending_smooth;
			pending = false;
			pending_smooth = false;
			published_smooth = smooth;
			revision += 1;
		};

		request(true);
		wake_queued = false;
		request(false);
		publish();

		expect(revision).toBe(1);
		expect(published_smooth).toBe(true);
		expect(wake_queued).toBe(true);
	});

	it("coalesces sustained churn into a later publish instead of dropping it", () => {
		let pending = false;
		let wake_queued = false;
		let revision = 0;

		const request = () => {
			pending = true;
			wake_queued ||= true;
		};
		const publish = () => {
			if (!pending) return;
			pending = false;
			revision += 1;
		};

		for (let index = 0; index < 1_000; index += 1) request();
		wake_queued = false;
		publish();
		request();
		wake_queued = false;
		publish();

		expect(revision).toBe(2);
		expect(pending).toBe(false);
	});
});
