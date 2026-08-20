import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const source = readFileSync(
	resolve(
		import.meta.dirname,
		"../../modules/frontend/src/routes/components/new-thread-route.svelte",
	),
	"utf8",
);

describe("new thread layout", () => {
	it("centres the introduction in the viewport at prose width", () => {
		expect(source).toContain(
			'class="prose-column-frame absolute inset-0 flex flex-col items-center justify-center"',
		);
		expect(source).toContain(
			'class="flex w-full max-w-(--prose-width) flex-col gap-3 text-center"',
		);
		expect(source).not.toContain('class="prose-column flex w-full max-w-(--prose-width)');
		expect(source).not.toContain("justify-center pb-40");
	});

	it("uses the first-project call to action without explanatory copy", () => {
		expect(source).toContain(
			"NewThreadSentenceWords(`Create your first ${ProjectMarker} today.`)",
		);
		expect(source).not.toContain("Nothing is attached yet");
	});
});
