import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("thread image attachment loading", () => {
	it("admits visible image loads concurrently with a bounded Effect scheduler", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const visible = route.slice(
			route.indexOf("const LoadVisibleImageAttachments"),
			route.indexOf("const HideImageAttachments"),
		);

		expect(visible).toContain(
			"for (const attachment of attachments) visible_image_ids.add(attachment.id);",
		);
		expect(visible).toContain("Effect.forEach(attachments, RequestImageAttachment, {");
		expect(visible).toContain("concurrency: 4,");
		expect(visible).toContain("discard: true,");
		expect(visible).not.toMatch(
			/for \(const attachment of attachments\)[\s\S]*yield\* RequestImageAttachment/u,
		);
	});

	it("keeps every hidden attachment's cleanup and revocation deterministic", () => {
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const hidden = route.slice(
			route.indexOf("const HideImageAttachments"),
			route.indexOf("const UpdateImageAttachmentVisibility"),
		);
		const update = route.slice(
			route.indexOf("const UpdateImageAttachmentVisibility"),
			route.indexOf("const ReplaceSnapshot"),
		);

		expect(hidden).toContain("visible_image_ids.delete(attachment.id);");
		expect(hidden).toContain("yield* ClearImageLoadState(attachment.id);");
		expect(hidden).toContain("yield* ReleaseImageAttachment(attachment.id);");
		expect(update).toContain("return yield* LoadVisibleImageAttachments(attachments);");
		expect(update).toContain("yield* HideImageAttachments(attachments);");
	});
});
