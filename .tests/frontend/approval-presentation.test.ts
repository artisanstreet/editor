import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ConversationItem } from "@artisan/protocol";
import { GetApprovalPresentation } from "../../modules/frontend/src/lib/conversation/approval-presentation";

const at = "2026-07-27T12:00:00.000Z";
const DecodeApproval = (value: unknown) => {
	const item = Schema.decodeUnknownSync(ConversationItem)(value);
	if (item.type !== "approval") throw new Error("Expected approval item");
	return item;
};

const base = {
	created_at: at,
	id: "approval_item",
	interaction_id: "opaque_response_id",
	lifecycle: "waiting",
	ordinal: 1,
	references: [],
	requested_at: at,
	revision: 0,
	source_refs: [],
	state: "requested",
	turn_id: "turn_1",
	type: "approval",
	updated_at: at,
};

describe("approval presentation", () => {
	it("presents a command decision without exposing its response handle", () => {
		const presentation = GetApprovalPresentation(
			DecodeApproval({
				...base,
				prompt: "Run the test suite",
				request: {
					command: "pnpm test",
					cwd: "C:\\workspace",
					kind: "command",
					reason: "Run the test suite",
				},
			}),
		);

		expect(presentation).toEqual({
			approve_label: "Run command",
			command: "pnpm test",
			cwd: "C:\\workspace",
			description: "Run the test suite",
			kind: "command",
			title: "Run this command?",
		});
		expect(JSON.stringify(presentation)).not.toContain("opaque_response_id");
	});

	it("suppresses legacy protocol plumbing and collapses a resolved decision", () => {
		const legacy = GetApprovalPresentation(
			DecodeApproval({
				...base,
				prompt: "item/commandExecution/requestApproval for call_U7RWgum9KP6PdckOOafVgxhi",
			}),
		);
		const resolved = GetApprovalPresentation(
			DecodeApproval({
				...base,
				lifecycle: "completed",
				prompt: "Run the test suite",
				request: {
					command: "pnpm test",
					kind: "command",
					reason: "Run the test suite",
				},
				resolution: "Approved",
				resolved_at: at,
				state: "approved",
			}),
		);

		expect(legacy).toMatchObject({
			description: "Artisan needs your approval before it can continue.",
			title: "Allow this action?",
		});
		expect(JSON.stringify(legacy)).not.toContain("requestApproval");
		expect(resolved).toEqual({
			approve_label: "Run command",
			command: "pnpm test",
			kind: "command",
			title: "Command approved",
		});
	});

	it("uses file-change language without inventing unavailable diff details", () => {
		expect(
			GetApprovalPresentation(
				DecodeApproval({
					...base,
					prompt: "Apply the generated fixes",
					request: {
						kind: "file_change",
						reason: "Apply the generated fixes",
					},
				}),
			),
		).toEqual({
			approve_label: "Apply changes",
			description: "Apply the generated fixes",
			kind: "file_change",
			title: "Apply these changes?",
		});
	});

	it("keeps the opaque response handle out of DOM identity and gates duplicate decisions", () => {
		const source = readFileSync(
			resolve("modules/frontend/src/routes/components/conversation-approval.sv"),
			"utf8",
		);

		expect(source).toContain("`approval-title-${item.ordinal}`");
		expect(source).not.toContain("approval-title-${item.id}");
		expect(source).toContain("submitted_decision !== undefined");
		expect(source).toContain("Could not respond to approval");
		expect(source).toContain("submitted_decision = undefined");
	});
});
