import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	AppearanceState,
	DefaultAppearanceState,
} from "../../modules/frontend/src/lib/runtime/appearance-preferences";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("prose width", () => {
	it("keeps a stored appearance from before the field instead of repairing it away", () => {
		const decoded = Schema.decodeUnknownSync(AppearanceState)({
			version: 1,
			shader_enabled: false,
		});

		expect(decoded).toEqual({ shader_enabled: false, version: 1 });
		expect(DefaultAppearanceState.prose_width).toBe("balanced");
	});

	it("defines the column tokens once and sizes every prose surface from them", () => {
		const tokens = ReadSource("modules/frontend/src/lib/styles/global.css");
		expect(tokens).toContain("--prose-width: 48rem;");
		expect(tokens).toContain("--prose-body-width: calc(var(--prose-width) - 6rem);");
		expect(tokens).toContain('[data-prose-width="tight"]');
		expect(tokens).toContain('[data-prose-width="loose"]');

		for (const [path, expected] of [
			[
				"modules/frontend/src/routes/components/thread-workspace.svelte",
				"max-w-(--prose-width)",
			],
			[
				"modules/frontend/src/routes/components/thread-composer.svelte",
				"max-w-(--prose-width)",
			],
			[
				"modules/frontend/src/routes/components/conversation-message.svelte",
				"max-w-(--prose-body-width)",
			],
		] as const) {
			expect(ReadSource(path)).toContain(expected);
		}
	});

	/**
	 * The workspace identity anchors to the primary card's own left edge on both
	 * surfaces — not the prose column's, whose title floats unexplained in a
	 * wide window. The stored width preference still reaches the shell as the
	 * data attribute the prose tokens read, and stays tunable from settings.
	 */
	it("anchors the workspace header to the card edge while prose stays tunable", () => {
		const layout = ReadSource("modules/frontend/src/routes/+layout.svelte");
		const panel = ReadSource("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const settings = ReadSource(
			"modules/frontend/src/routes/components/settings/appearance.svelte",
		);

		expect(layout).toContain("data-prose-width={$prose_width}");
		expect(layout).toContain("the primary card's left edge");
		expect(panel).toContain("the card's left edge");
		expect(settings).toContain("SelectProseWidth");
	});
});
