import { existsSync, readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(path, "utf8");
const dialog_path = existsSync(
	"modules/frontend/src/routes/components/project-attach-dialog.svelte",
)
	? "modules/frontend/src/routes/components/project-attach-dialog.svelte"
	: "modules/frontend/src/routes/components/project-folder-picker.svelte";

describe("project native picker", () => {
	it("asks Forge for the native choice and resolves only its opaque directory id", () => {
		const dialog = Read(dialog_path);

		expect(dialog).toContain("client.PickProjectDirectory");
		expect(dialog).toContain("picked.directory.directory_id");
		expect(dialog).toContain("client.SelectProjectDirectory({ directory_id })");
		expect(dialog).not.toMatch(/showDirectoryPicker|webkitdirectory|picked\.path/u);
	});

	it("falls back to Forge's opaque directory browser only when native picking fails", () => {
		const dialog = Read(dialog_path);

		expect(dialog).toContain("if (picked === undefined)");
		expect(dialog).toMatch(/yield\* (?:Load\(undefined\)|Browse\(\[\]\))/u);
		expect(dialog).toContain('if (picked.status === "cancelled")');
	});
});
