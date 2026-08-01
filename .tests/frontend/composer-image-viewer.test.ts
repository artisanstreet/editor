import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("composer image viewer", () => {
	it("keeps orchestration below the component pressure limit", () => {
		const composer = ReadSource("modules/frontend/src/routes/components/thread-composer.sv");

		expect(composer.split(/\r?\n/u).length).toBeLessThanOrEqual(560);
	});

	it("uses the shared card treatment for both image entry points without custom borders", () => {
		const composer = ReadSource("modules/frontend/src/routes/components/thread-composer.sv");
		const composer_dom = ReadSource("modules/frontend/src/routes/components/composer/dom.ts");
		const tray = ReadSource(
			"modules/frontend/src/routes/components/composer/attachment-tray.sv",
		);

		expect(tray).toContain('class="composer-attachment-preview card"');
		expect(composer_dom).toContain('marker.className = "composer-image-marker card"');
		expect(composer).not.toContain("border: 1px solid rgb(255 255 255 / .16)");
		expect(composer).not.toContain("border: 1px solid rgb(255 255 255 / .38)");
	});

	it("opens previews rather than removing an inline attachment marker", () => {
		const composer = ReadSource("modules/frontend/src/routes/components/thread-composer.sv");
		const tray = ReadSource(
			"modules/frontend/src/routes/components/composer/attachment-tray.sv",
		);

		expect(composer).toContain("const ViewAttachment = (attachment: ComposerImageAttachment)");
		expect(composer).toContain(
			"if (attachment !== undefined) yield* ViewAttachment(attachment);",
		);
		expect(tray).toContain("aria-label={`View ${attachment.name}`}");
	});

	it("uses an accessible blurred dialog that never upscales the image", () => {
		const viewer = ReadSource("modules/frontend/src/routes/components/image-viewer.sv");

		expect(viewer).toContain("DialogPrimitive.Root bind:open");
		expect(viewer).toContain("supports-backdrop-filter:backdrop-blur-md");
		expect(viewer).toContain("p-8");
		expect(viewer).toContain("h-auto w-auto max-h-full max-w-full object-contain");
		expect(viewer).toContain("DialogPrimitive.Close");
		expect(viewer).toContain("event.currentTarget === event.target");
		expect(viewer).toContain("const titlebar_overlay_height");
		expect(viewer).toContain('globalThis.navigator?.userAgent.includes("Electron/")');
		expect(viewer).toContain("--titlebar-overlay-height,0px");
	});

	it("releases a viewed image before its URL can be revoked by keyboard deletion", () => {
		const composer = ReadSource("modules/frontend/src/routes/components/thread-composer.sv");

		expect(composer).toContain("if (viewed_attachment?.id === attachment_id)");
		expect(composer).toContain("viewed_attachment = undefined;");
		expect(composer).toContain("onclose={ClearViewedAttachment}");
	});

	it("yields every mutable browser boundary through the shared DOM adapter", () => {
		const composer = ReadSource("modules/frontend/src/routes/components/thread-composer.sv");
		const composer_dom = ReadSource("modules/frontend/src/routes/components/composer/dom.ts");

		expect(composer).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(composer).toContain('from "./composer/dom"');
		expect(composer_dom).toContain("export const ReadComposerEditorDocument");
		expect(composer).toContain("const InsertAttachments");
		expect(composer_dom).toContain("export const RemoveComposerAttachmentMarkers");
		expect(composer_dom).toContain("export const FocusComposerRange");
		expect(composer_dom).toContain("export const ComposerDropRange");
		expect(composer).toContain("yield* RunBrowserDom(() => event.preventDefault())");
		expect(composer).toContain("yield* ReleaseBrowserObjectUrl(attachment.preview_url)");
		expect(composer_dom).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(composer_dom).toContain("document.createElement");
		expect(composer).not.toContain("Effect.callback");
		expect(composer).not.toContain("Effect.void");
	});

	it("keeps FileReader lifecycle operations inside typed browser boundaries", () => {
		const reader = ReadSource("modules/frontend/src/lib/composer/attachment-reader.ts");

		expect(reader).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(reader).toContain("const RunAttachmentBrowser");
		expect(reader).toContain("const AwaitFileReader");
		expect(reader).toContain("yield* RunAttachmentBrowser(file, () => new FileReader())");
		expect(reader).toContain("yield* RunAttachmentBrowser(file, () => reader.abort())");
		expect(reader).toContain("URL.createObjectURL(file)");
		expect(reader).toContain("AttachmentReadError");
		expect(reader).not.toContain("Effect.void");
	});
});
