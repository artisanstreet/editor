import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("persisted conversation image attachments", () => {
	it("accepts caller-owned resolved image sources and renders only resolved attachments", () => {
		const source = ReadSource("modules/frontend/src/routes/components/conversation-message.sv");

		expect(source).toContain("image_sources?: ReadonlyMap<string, string>");
		expect(source).toContain("const source = image_sources?.get(attachment.id);");
		expect(source).toContain("return source === undefined ? [] : [{ attachment, source }];");
	});

	it("groups compact card thumbnails with the user message and reuses their source in the viewer", () => {
		const source = ReadSource("modules/frontend/src/routes/components/conversation-message.sv");

		expect(source).toContain('class="card conversation-image-thumbnail"');
		expect(source).toContain("aria-label={`View ${image.attachment.name}`}");
		expect(source).toContain("source={viewed_image?.source}");
		expect(source).toContain("IntersectionObserver");
		expect(source).toContain('{ rootMargin: "160px 0px" }');
		expect(source).toContain('globalThis.addEventListener("scroll", measure, true)');
		expect(source).toContain("if (visible || !image_viewer_open)");
		expect(source).toContain("if (!image_group_visible");
		expect(source).toContain("onimagevisibilitychange?.(attachments, false)");
		expect(source).not.toContain("/api/attachments/");
		expect(source).not.toContain("border: 1px");
		expect(source.indexOf('aria-label="Attached images"')).toBeLessThan(
			source.indexOf('class="user-message max-w-full rounded-2xl px-4 py-3"'),
		);
	});

	it("loads thread-scoped bytes once and revokes every object URL with the route scope", () => {
		const route = ReadSource("modules/frontend/src/routes/components/thread-route.sv");

		expect(route).toContain("client.GetMessageImageAttachment({");
		expect(route).toContain("requested_image_ids.has(attachment.id)");
		expect(route).toContain("URL.createObjectURL(");
		expect(route).toContain("Scope.addFinalizer(");
		expect(route).toContain("URL.revokeObjectURL(source)");
		expect(route).toContain("visible_image_ids.has(attachment.id)");
		expect(route).toContain("attempt >= 3");
		expect(route).toContain("Effect.sleep(attempt * 500)");
		expect(route).toContain("image_load_attempts.delete(attachment.id)");
		expect(route).toContain("onimagevisibilitychange={UpdateImageAttachmentVisibility}");
	});
});
