import { describe, expect, it } from "vitest";
import { readFile } from "node:fs/promises";

const card_path = new URL(
	"../../modules/frontend/src/routes/components/conversation-changes-card.svelte",
	import.meta.url,
);
const thread_workspace_path = new URL(
	"../../modules/frontend/src/routes/components/thread-workspace.svelte",
	import.meta.url,
);
const thread_route_path = new URL(
	"../../modules/frontend/src/routes/components/thread-route.svelte",
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

	it("receives the active project root and uses it only for the visible path", async () => {
		const [card, thread_workspace, thread_route] = await Promise.all([
			readFile(card_path, "utf8"),
			readFile(thread_workspace_path, "utf8"),
			readFile(thread_route_path, "utf8"),
		]);

		expect(thread_route).toContain("project_root_path={thread?.primary_project?.root_path}");
		expect(thread_workspace).toContain("{project_root_path}");
		expect(card).toContain("display_file_change_path(file.path, project_root_path)");
		expect(card).toContain("CopyPath(file.path)");
	});
});
