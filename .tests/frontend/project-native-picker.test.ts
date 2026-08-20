import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const picker = readFileSync(
	"modules/frontend/src/routes/components/project-folder-picker.svelte",
	"utf8",
);

describe("project native picker", () => {
	it("asks Forge for the native choice and resolves only its opaque directory id", () => {
		expect(picker).toContain("client.PickProjectDirectory");
		expect(picker).toContain("picked.directory.directory_id");
		expect(picker).toContain("client.SelectProjectDirectory({ directory_id })");
		expect(picker).not.toMatch(/showDirectoryPicker|webkitdirectory|picked\.path/u);
	});

	it("closes on refusal or cancellation instead of opening a browser of its own", () => {
		expect(picker).toContain('picked === undefined || picked.status === "cancelled"');
		expect(picker).not.toContain("ListProjectDirectories");
		expect(picker).not.toMatch(/yield\* (?:Load\(undefined\)|Browse\(\[\]\))/u);
	});
});
