import { describe, expect, it } from "vitest";

import type { ThreadSessionPolicy } from "@artisan/protocol";
import { MakeSessionToolPolicy } from "../../modules/frontend/src/lib/live-workspace/session-tool-policy";

const SessionPolicy = (overrides: Partial<ThreadSessionPolicy> = {}): ThreadSessionPolicy => ({
	engine_id: "codex",
	permission_mode: "on_request",
	reasoning_effort: "medium",
	sandbox_mode: "workspace_write",
	service_tier: "standard",
	strict_clarification: false,
	web_search_enabled: false,
	...overrides,
});

describe("session tool policy", () => {
	it("fails closed for mutations until the session policy is authoritative", () => {
		expect(MakeSessionToolPolicy(undefined)).toEqual({
			approval: "never",
			allow_engine_observation: true,
			allow_git_index_write: false,
			allow_preview_control: false,
			allow_process_control: false,
			allow_workspace_read: true,
			allow_workspace_write: false,
		});
	});

	it("does not advertise mutating tools for a read-only session", () => {
		expect(
			MakeSessionToolPolicy(
				SessionPolicy({ permission_mode: "never", sandbox_mode: "read_only" }),
			),
		).toMatchObject({
			approval: "never",
			allow_git_index_write: false,
			allow_preview_control: false,
			allow_process_control: false,
			allow_workspace_write: false,
		});
	});

	it("advertises write/control tools only for a workspace-write session", () => {
		expect(MakeSessionToolPolicy(SessionPolicy())).toMatchObject({
			approval: "on_request",
			allow_git_index_write: true,
			allow_preview_control: true,
			allow_process_control: true,
			allow_workspace_write: true,
		});
	});
});
