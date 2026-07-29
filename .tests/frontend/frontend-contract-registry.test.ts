import { describe, expect, it } from "vitest";

import { FrontendContractRegistry } from "../../modules/frontend/src/lib/contracts/frontend-contract-registry";

describe("frontend contract registry", () => {
	it("classifies every initial shell interaction with a unique stable id", () => {
		const ids = FrontendContractRegistry.map((entry) => entry.id);

		expect(new Set(ids).size).toBe(ids.length);
		expect(ids).toEqual(
			expect.arrayContaining([
				"left.thread-list",
				"left.thread-create",
				"left.thread-rename",
				"left.thread-pin",
				"left.thread-archive",
				"left.thread-project",
				"left.thread-retention",
				"left.identity",
				"left.marketplace",
				"main.workspace-modes",
				"main.file-tabs",
				"main.current-work",
				"main.activity-status",
				"main.transcript",
				"main.chat-send",
				"main.chat-steer",
				"main.approval",
				"main.question",
				"main.orchestration-known-id",
				"main.file-discovery",
				"main.file-read",
				"main.file-replace",
				"main.change-diff",
				"main.change-review",
				"main.change-rollback",
				"right.terminals",
				"right.compact-agents",
				"right.session-policy",
				"right.git",
				"right.processes-ports",
				"right.previews",
				"right.permissions-usage",
				"connection.lifecycle",
				"desktop-shell.electron-bootstrap",
			]),
		);
		for (const entry of FrontendContractRegistry) {
			expect(entry.id).toMatch(/^[a-z0-9]+(?:[.-][a-z0-9]+)*$/);
			expect(entry.pane.length).toBeGreaterThan(0);
			expect(entry.reason.length).toBeGreaterThan(0);
		}
	});

	it("names every public contract claimed by typed-client-owned live behavior", () => {
		for (const entry of FrontendContractRegistry) {
			if (entry.state === "live" && entry.owner === "artisan_client") {
				expect(entry.contract_names, entry.id).toBeDefined();
				expect(entry.contract_names?.length, entry.id).toBeGreaterThan(0);
				for (const contract_name of entry.contract_names ?? []) {
					expect(contract_name, entry.id).toMatch(
						/^[A-Z][A-Za-z0-9]*(?:\.[A-Z][A-Za-z0-9]*)?$/,
					);
				}
			}
		}
	});

	it("never assigns moving or blocked behavior to the typed client", () => {
		for (const entry of FrontendContractRegistry) {
			if (entry.state === "backend_moving" || entry.state === "blocked") {
				expect(entry.owner, entry.id).not.toBe("artisan_client");
				expect(entry.contract_names, entry.id).toBeUndefined();
			}
		}
	});

	it("keeps local shell behavior live and frontend-owned", () => {
		for (const id of [
			"left.thread-select",
			"main.workspace-modes",
			"main.file-tabs",
			"main.pane-behaviour",
		]) {
			const entry = FrontendContractRegistry.find((candidate) => candidate.id === id);

			expect(entry).toMatchObject({ owner: "frontend", state: "live" });
		}
	});

	it("records the audited integration state for moving and unavailable domains", () => {
		const expected_states = {
			"desktop-shell.electron-bootstrap": "live",
			"left.identity": "live",
			"left.marketplace": "live",
			"main.change-diff": "live",
			"main.change-review": "live",
			"main.change-rollback": "live",
			"main.chat-send": "live",
			"main.chat-steer": "live",
			"main.file-read": "live",
			"main.file-replace": "live",
			"main.transcript": "live",
			"main.activity-status": "live",
			"right.git": "live",
			"right.session-policy": "live",
			"right.terminals": "live",
		} as const;

		for (const [id, state] of Object.entries(expected_states)) {
			expect(
				FrontendContractRegistry.find((entry) => entry.id === id),
				id,
			).toMatchObject({
				state,
			});
		}

		expect(
			FrontendContractRegistry.some(
				(entry) => entry.id === "desktop-shell.activity-indicator",
			),
		).toBe(false);
	});

	it("records the one-visible-checkout assignment constraint", () => {
		const entry = FrontendContractRegistry.find(
			(candidate) => candidate.id === "main.orchestration-start",
		);
		const guidance = entry?.guidance?.join(" ") ?? "";

		expect(guidance).toContain("isolation set to shared");
		expect(guidance).toContain("Do not expose the isolated");
	});
});
