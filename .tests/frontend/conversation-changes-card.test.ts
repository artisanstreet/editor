import { describe, expect, it } from "vitest";
import { readFile } from "node:fs/promises";

const card_path = new URL(
	"../../modules/frontend/src/routes/components/conversation-changes-card.svelte",
	import.meta.url,
);

describe("conversation changes card layout", () => {
	it("aligns file rows with the card header and uses the row rhythm below it", async () => {
		const source = await readFile(card_path, "utf8");

		expect(source).toContain("flex w-full flex-col gap-1.5");
		expect(source).toContain('class="flex items-center justify-between gap-4"');
		expect(source).toContain("items-center justify-between gap-4 py-1.5 text-left");
		expect(source).not.toContain("items-center justify-between gap-4 px-2 py-1.5");
	});
});
