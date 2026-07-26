import { describe, expect, it } from "vitest";

import { GetConversationActivityPresentation } from "@artisan/protocol";

const Present = (kind: string, status: "active" | "completed" | "failed" = "completed") =>
	GetConversationActivityPresentation({ kind, label: "Provider fallback", status }).label;

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

	it("preserves a safe normalized label for unknown future semantics", () => {
		expect(Present("future.provider.action")).toBe("Provider fallback");
	});
});
