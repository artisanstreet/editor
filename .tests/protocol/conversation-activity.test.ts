import { describe, expect, it } from "vitest";

import {
	GetConversationActivityCategory,
	GetConversationActivityCountLabel,
	GetConversationActivityGroupPresentation,
	GetConversationActivityPresentation,
} from "@artisan/protocol";

const Present = (kind: string, status: "active" | "completed" | "failed" = "completed") =>
	GetConversationActivityPresentation({ kind, label: "Provider fallback", status }).label;

const Counted = (kind: string, count: number) =>
	GetConversationActivityCountLabel(GetConversationActivityCategory(kind), count);

describe("conversation activity presentation", () => {
	it("maps equivalent engine and fixture semantics to universal labels", () => {
		expect(Present("terminal_activity")).toBe("Ran a command");
		expect(Present("terminal.command")).toBe("Ran a command");
		expect(Present("file")).toBe("Read a file");
		expect(Present("workspace.read")).toBe("Read a file");
		expect(Present("search")).toBe("Searched the web");
		expect(Present("tool")).toBe("Used a tool");
		expect(Present("test.run")).toBe("Ran tests");
	});

	it("reflects active and failed lifecycle states without provider-specific wording", () => {
		expect(Present("terminal_activity", "active")).toBe("Running a command");
		expect(Present("workspace.read", "failed")).toBe("File read failed");
		expect(Present("web_search", "active")).toBe("Searching the web");
	});

	it("uses the durable subagent name for individual lifecycle rows", () => {
		const subagent = { agent_id: "agent-randi", display_name: "Randi" };
		expect(
			GetConversationActivityPresentation({
				kind: "subagent",
				label: "Talked to subagent",
				status: "active",
				subagent,
			}).label,
		).toBe("Talking to Randi");
		expect(
			GetConversationActivityPresentation({
				kind: "subagent",
				label: "Talked to subagent",
				status: "completed",
				subagent,
			}).label,
		).toBe("Talked to Randi");
	});

	it("preserves a safe normalized label for unknown future semantics", () => {
		expect(Present("future.provider.action")).toBe("Provider fallback");
	});

	/**
	 * A grouped chain describes itself by composition rather than by a bare total,
	 * so each category contributes a lowercase clause the caller joins and
	 * capitalises once. Both readings key off the same classification, which is
	 * what stops "Ran a command" and its counted form from drifting apart.
	 */
	it("counts a category in the same words its single activity uses", () => {
		expect(Counted("terminal_activity", 1)).toBe("ran a command");
		expect(Counted("terminal_activity", 4)).toBe("ran 4 commands");
		expect(Counted("workspace.edit", 1)).toBe("edited a file");
		expect(Counted("workspace.edit", 2)).toBe("edited 2 files");
		expect(Counted("search", 1)).toBe("searched the web");
		expect(Counted("search", 3)).toBe("ran 3 web searches");
		expect(Counted("workspace.search", 1)).toBe("searched files");
		expect(Counted("workspace.search", 5)).toBe("searched 5 files");
		expect(Counted("test.run", 3)).toBe("ran 3 test runs");
		expect(Counted("typecheck", 2)).toBe("ran 2 type checks");
		expect(Counted("future.provider.action", 5)).toBe("used 5 tools");
	});

	it("classifies narrower semantics before the broad tool catch", () => {
		expect(GetConversationActivityCategory("mcp.tool.call")).toBe("integration");
		expect(GetConversationActivityCategory("tool")).toBe("tool");
		expect(GetConversationActivityCategory("future.provider.action")).toBe("other");
	});

	/**
	 * The projected trace rows for file mutations carry these kinds, so a chain
	 * that edited and deleted counts itself as that work rather than as tools.
	 */
	it("classifies projected file mutation rows as the work they did", () => {
		expect(GetConversationActivityCategory("file_edit")).toBe("file_edit");
		expect(GetConversationActivityCategory("file_delete")).toBe("file_delete");
		expect(Counted("file_edit", 2)).toBe("edited 2 files");
		expect(Counted("file_delete", 1)).toBe("deleted a file");
	});

	it("names one durable subagent and counts several distinct agents", () => {
		expect(
			GetConversationActivityGroupPresentation("subagent", [
				{
					kind: "subagent",
					subagent: { agent_id: "agent-noodle", display_name: "Noodle" },
				},
			]),
		).toEqual({ count: 1, label: "talked to Noodle" });

		expect(
			GetConversationActivityGroupPresentation("subagent", [
				{
					kind: "subagent",
					subagent: { agent_id: "agent-noodle", display_name: "Noodle" },
				},
				{
					kind: "subagent",
					subagent: { agent_id: "agent-noodle", display_name: "Noodle" },
				},
				{
					kind: "subagent",
					subagent: { agent_id: "agent-sprocket", display_name: "Sprocket" },
				},
			]),
		).toEqual({ count: 2, label: "talked to 2 subagents" });
	});
});
