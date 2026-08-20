import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { ReadStylesheets } from "./stylesheet-source";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("composer image viewer", () => {
	it("keeps orchestration below the component pressure limit", () => {
		const composer = ReadSource(
			"modules/frontend/src/routes/components/thread-composer.svelte",
		);

		expect(composer.split(/\r?\n/u).length).toBeLessThanOrEqual(560);
	});

	it("uses the shared card treatment for both image entry points without custom borders", () => {
		const composer = ReadSource(
			"modules/frontend/src/routes/components/thread-composer.svelte",
		);
		const composer_dom = ReadSource("modules/frontend/src/routes/components/composer/dom.ts");
		const styles = ReadStylesheets();
		const tray = ReadSource(
			"modules/frontend/src/routes/components/composer/attachment-tray.svelte",
		);

		expect(tray).toContain('class="card relative size-18 flex-none');
		expect(composer_dom).toContain('marker.className = "composer-image-marker card"');
		for (const source of [composer, styles]) {
			expect(source).not.toContain("border: 1px solid rgb(255 255 255 / .16)");
			expect(source).not.toContain("border: 1px solid rgb(255 255 255 / .38)");
		}
	});

	/**
	 * A pasted image is shown at once and encoded behind it. The wait is measured
	 * in milliseconds, so nothing about it is dressed up: no blur, no spinner, and
	 * no disarming of the send button — a submit that lands early just declines.
	 */
	it("shows a pasted image immediately and declines an early send silently", () => {
		const composer = ReadSource(
			"modules/frontend/src/routes/components/thread-composer.svelte",
		);
		const tray = ReadSource(
			"modules/frontend/src/routes/components/composer/attachment-tray.svelte",
		);
		const styles = ReadStylesheets();

		expect(composer).toContain("if (!attachments_ready) return;");
		/** Arming must not depend on readiness, or the button would flicker. */
		expect(composer).not.toContain("attachments_ready &&");
		for (const source of [composer, tray, styles]) {
			expect(source).not.toContain("image-settle");
		}
		expect(tray).not.toContain("aria-busy");
	});

	/** A duplicate remains refused, but paste never shakes the composer or either image. */
	it("keeps pasted images still while discarding duplicates", () => {
		const composer = ReadSource(
			"modules/frontend/src/routes/components/thread-composer.svelte",
		);
		const tray = ReadSource(
			"modules/frontend/src/routes/components/composer/attachment-tray.svelte",
		);
		const styles = ReadStylesheets();
		const intake = ReadSource("modules/frontend/src/lib/composer/attachment-intake.ts");
		const composer_dom = ReadSource("modules/frontend/src/routes/components/composer/dom.ts");
		const motion_path = resolve(
			import.meta.dirname,
			"../../modules/frontend/src/routes/components/composer/motion.ts",
		);

		expect(intake).toContain("const attached = FindDuplicateImageAttachment(");
		expect(intake).toContain("yield* surface.Revoke(attachment);");
		expect(intake).not.toContain("Bump");
		expect(composer).not.toContain("MakeComposerMotion");
		expect(composer).not.toContain("data-rustle");
		expect(tray).not.toContain("t-rustle");
		expect(tray).not.toContain("data-bump");
		expect(composer_dom).not.toContain("MarkComposerAttachmentBumps");
		expect(styles).not.toContain('.composer-image-marker[data-bump="true"]');
		expect(existsSync(motion_path)).toBe(false);
	});

	/** Everything except the image dismisses the viewer, and the rail stands down. */
	it("closes the image viewer from anywhere but the image", () => {
		const viewer = ReadSource("modules/frontend/src/routes/components/image-viewer.svelte");
		const panel = ReadSource("modules/frontend/src/routes/components/sectioned-panel.svelte");

		/** The dialog fills the viewport, so dismissal is owned here, not by the primitive. */
		expect(viewer).toContain('class="absolute inset-0 cursor-default"');
		expect(viewer).toContain("onclick={DismissViewer}");
		expect(viewer).toContain("const DismissViewer = () => {");
		/** The image paints above the dismiss layer, so clicking it keeps the viewer open. */
		expect(viewer).toContain(
			'class="group/image relative z-10 inline-flex max-h-full max-w-full"',
		);
		expect(viewer).toContain("yield* inspection.Retain");
		expect(panel).toContain("suppressed={account_open || inspecting_image}");
	});

	it("opens previews rather than removing an inline attachment marker", () => {
		const composer = ReadSource(
			"modules/frontend/src/routes/components/thread-composer.svelte",
		);
		const tray = ReadSource(
			"modules/frontend/src/routes/components/composer/attachment-tray.svelte",
		);

		expect(composer).toContain("const ViewAttachment = (attachment: ComposerImageAttachment)");
		expect(composer).toContain(
			"if (attachment !== undefined) yield* ViewAttachment(attachment);",
		);
		expect(tray).toContain("aria-label={`View ${attachment.name}`}");
	});

	it("uses an accessible blurred dialog that never upscales the image", () => {
		const viewer = ReadSource("modules/frontend/src/routes/components/image-viewer.svelte");

		expect(viewer).toContain("DialogPrimitive.Root bind:open");
		expect(viewer).toContain("supports-backdrop-filter:backdrop-blur-md");
		expect(viewer).toContain("p-8");
		expect(viewer).toContain("h-auto w-auto max-h-full max-w-full object-contain");
		expect(viewer).toContain("DialogPrimitive.Close");
		expect(viewer).toContain('class="group/image relative z-10');
		expect(viewer).toContain("group-hover/image:opacity-100");
		expect(viewer).toContain("focus-within:opacity-100");
		expect(viewer).toContain("<ShaderGlassSurface");
		expect(viewer).toContain('variant="ghost"');
		expect(viewer).toContain('size="icon-sm"');
		expect(viewer).not.toContain('variant="secondary"');

		expect(viewer).toContain("const titlebar_overlay_height");
		expect(viewer).toContain('globalThis.navigator?.userAgent.includes("Electron/")');
		expect(viewer).toContain("--titlebar-overlay-height,0px");
	});

	it("releases a viewed image before its URL can be revoked by keyboard deletion", () => {
		const composer = ReadSource(
			"modules/frontend/src/routes/components/thread-composer.svelte",
		);

		expect(composer).toContain("if (viewed_attachment?.id === attachment_id)");
		expect(composer).toContain("viewed_attachment = undefined;");
		expect(composer).toContain("onclose={ClearViewedAttachment}");
	});

	it("yields every mutable browser boundary through the shared DOM adapter", () => {
		const composer = ReadSource(
			"modules/frontend/src/routes/components/thread-composer.svelte",
		);
		const composer_dom = ReadSource("modules/frontend/src/routes/components/composer/dom.ts");

		expect(composer).toContain('from "./composer/dom"');
		expect(composer_dom).toContain("export const ReadComposerEditorDocument");
		expect(composer).toContain("const InsertAttachments");
		expect(composer_dom).toContain("export const RemoveComposerAttachmentMarkers");
		expect(composer_dom).toContain("export const FocusComposerRange");
		expect(composer_dom).toContain("export const ComposerDropRange");
		expect(composer).toContain("yield* ReleaseBrowserObjectUrl(attachment.preview_url)");
		expect(composer_dom).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(composer_dom).toContain("document.createElement");
		expect(composer).not.toContain("Effect.callback");
		expect(composer).not.toContain("Effect.void");
	});

	/**
	 * A SER event effect starts after its event finishes dispatching, by which
	 * time the clipboard transfer is empty and `preventDefault` is a no-op. The
	 * gestures that depend on that window must stay plain Svelte handlers owned
	 * by the intake, never `yield*` sites.
	 */
	it("captures expiring gesture state synchronously instead of inside an effect", () => {
		const composer = ReadSource(
			"modules/frontend/src/routes/components/thread-composer.svelte",
		);
		const intake = ReadSource("modules/frontend/src/lib/composer/gesture-intake.ts");

		for (const handler of [
			"onkeydown={gestures.SubmitKey}",
			"onpaste={gestures.Paste}",
			"ondragover={gestures.AcceptFileDrag}",
			"ondrop={gestures.Drop}",
		]) {
			expect(composer).toContain(handler);
		}
		expect(composer).toContain("const gestures = yield* MakeComposerGestureIntake(");
		expect(composer).not.toContain("event.preventDefault()");
		expect(composer).not.toContain("clipboardData");
		expect(composer).not.toContain("dataTransfer");

		expect(intake).toContain("export const MakeComposerGestureIntake");
		expect(intake).toContain("yield* Queue.take(gestures)");
		for (const capture of [
			"event.clipboardData",
			"event.dataTransfer",
			"event.preventDefault()",
			"Queue.offerUnsafe(gestures",
		]) {
			expect(intake).toContain(capture);
		}
	});

	it("keeps FileReader lifecycle operations inside typed browser boundaries", () => {
		const reader = ReadSource("modules/frontend/src/lib/composer/attachment-reader.ts");

		expect(reader).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(reader).toContain("const RunAttachmentBrowser");
		expect(reader).toContain("const AwaitFileReader");
		expect(reader).toContain("yield* RunAttachmentBrowser(encoded, () => new FileReader())");
		expect(reader).toContain("yield* RunAttachmentBrowser(file, () => reader.abort())");
		expect(reader).toContain("URL.createObjectURL(file)");
		expect(reader).toContain("AttachmentReadError");
		expect(reader).not.toContain("Effect.void");
	});
});
