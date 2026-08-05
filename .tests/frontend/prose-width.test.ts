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
			["modules/frontend/src/routes/components/thread-workspace.sv", "max-w-(--prose-width)"],
			["modules/frontend/src/routes/components/thread-composer.sv", "max-w-(--prose-width)"],
			[
				"modules/frontend/src/routes/components/conversation-message.sv",
				"max-w-(--prose-body-width)",
			],
		] as const) {
			expect(ReadSource(path)).toContain(expected);
		}
	});

	/**
	 * The workspace identity is left-aligned onto the prose column's own edge —
	 * in the desktop window frame and the web card band alike — so retuning the
	 * width in settings moves the title and the text it titles together.
	 */
	it("anchors the workspace header to the prose column on both surfaces", () => {
		const layout = ReadSource("modules/frontend/src/routes/+layout.sv");
		const panel = ReadSource("modules/frontend/src/routes/components/sectioned-panel.sv");
		const settings = ReadSource("modules/frontend/src/routes/components/settings/appearance.sv");

		expect(layout).toContain("data-prose-width={$prose_width}");
		expect(layout).toContain("max-w-(--prose-width)");
		expect(panel).toContain("max-w-(--prose-width)");
		expect(settings).toContain("SelectProseWidth");
	});
});
