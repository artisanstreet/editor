import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("composer image viewer", () => {
	it("uses the shared card treatment for both image entry points without custom borders", () => {
		const composer = ReadSource("modules/frontend/src/routes/components/thread-composer.sv");

		expect(composer).toContain('class="composer-attachment-preview card"');
		expect(composer).toContain('marker.className = "composer-image-marker card"');
		expect(composer).not.toContain("border: 1px solid rgb(255 255 255 / .16)");
		expect(composer).not.toContain("border: 1px solid rgb(255 255 255 / .38)");
	});

	it("opens previews rather than removing an inline attachment marker", () => {
		const composer = ReadSource("modules/frontend/src/routes/components/thread-composer.sv");

		expect(composer).toContain("const view_attachment = (attachment: ComposerImageAttachment)");
		expect(composer).toContain("if (attachment !== undefined) view_attachment(attachment);");
		expect(composer).toContain("aria-label={`View ${attachment.name}`}");
	});

	it("uses an accessible blurred dialog that never upscales the image", () => {
		const viewer = ReadSource("modules/frontend/src/routes/components/image-viewer.sv");

		expect(viewer).toContain("DialogPrimitive.Root bind:open");
		expect(viewer).toContain("supports-backdrop-filter:backdrop-blur-md");
		expect(viewer).toContain("p-8");
		expect(viewer).toContain("h-auto w-auto max-h-full max-w-full object-contain");
		expect(viewer).toContain("DialogPrimitive.Close");
		expect(viewer).toContain("event.currentTarget === event.target");
		expect(viewer).toContain('let titlebar_overlay_height = $state("0px")');
		expect(viewer).toContain('navigator.userAgent.includes("Electron/")');
		expect(viewer).toContain("--titlebar-overlay-height,0px");
	});

	it("releases a viewed image before its URL can be revoked by keyboard deletion", () => {
		const composer = ReadSource("modules/frontend/src/routes/components/thread-composer.sv");

		expect(composer).toContain("if (viewed_attachment?.id === id)");
		expect(composer).toContain("viewed_attachment = undefined;");
		expect(composer).toContain("onclose={() => {");
	});
});
