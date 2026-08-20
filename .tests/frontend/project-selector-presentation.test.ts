import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const selector = readFileSync(
	resolve(
		import.meta.dirname,
		"../../modules/frontend/src/routes/components/project-selector.svelte",
	),
	"utf8",
);
const mark = readFileSync(
	resolve(
		import.meta.dirname,
		"../../modules/frontend/src/routes/components/project-identity-mark.svelte",
	),
	"utf8",
);

describe("project selector presentation", () => {
	it("shows compact paths with the readable muted foreground token", () => {
		expect(selector).toContain("ShortProjectPath(");
		expect(selector).toContain("text-muted-foreground");
		expect(selector).toContain("title={recent.project.root_path}");
		expect(selector).not.toContain("text-surface-600");
	});

	it("uses fetched identity images, provider marks, and a folder fallback", () => {
		expect(selector).toContain("<ProjectIdentityMark");
		expect(selector).not.toContain("GradientAvatarColorFor");
		expect(selector).not.toContain("ProjectMonogram");
		expect(mark).toContain("assets.Load(next.image)");
		expect(mark).toContain("RepositoryMarkFor(identity.host)");
		expect(mark).toContain('<Folder class="size-3.5" />');
	});
});
