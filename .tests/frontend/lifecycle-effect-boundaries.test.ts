import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";

const ReadSource = (path: string) => readFile(new URL(`../../${path}`, import.meta.url), "utf8");

const ComponentPaths = [
	"modules/frontend/src/routes/components/conversation-message.svelte",
	"modules/frontend/src/routes/components/paper-god-rays.svelte",
] as const;

const DropdownCallerPaths = [
	"modules/frontend/src/routes/components/model-selector/policy-controls.svelte",
	"modules/frontend/src/routes/components/settings/compaction-model.svelte",
	"modules/frontend/src/routes/components/sidebar-identity.svelte",
] as const;

describe("frontend browser lifecycle ownership", () => {
	it.each(ComponentPaths)("%s is an SER component with scoped resources", async (path) => {
		const source = await ReadSource(path);

		expect(source).toContain('<script lang="ts" effect>');
		expect(source).toContain("Effect.acquireRelease");
		expect(source).not.toMatch(/\bEffect\.runFork\b|\brunFork\b/);
		expect(source).not.toMatch(/\bas unknown as\b|!\./);
		expect(source.split("\n").length).toBeLessThan(800);
	});

	it("owns dropdown attachments through the shared scoped attachment runner", async () => {
		const source = await ReadSource(
			"modules/frontend/src/lib/components/dropdown-highlight.ts",
		);

		expect(source).toContain("const AcquireHighlight");
		expect(source).toContain("Effect.acquireRelease");
		expect(source).toContain("MakeScopedAttachmentRunner");
		expect(source).toContain("highlight_runner.Attachment");
		expect(source).not.toContain("Queue.unbounded<HighlightCommand>()");
		expect(source).not.toMatch(
			/\bEffect\.run(?:Sync|Fork)\b|\brun(?:Sync|Fork)\b|\bScope\.(?:make|close)\b/,
		);
		expect(source).not.toMatch(/\bas unknown as\b|!\./);
	});

	it.each(DropdownCallerPaths)(
		"%s acquires its dropdown attachment through SER",
		async (path) => {
			const source = await ReadSource(path);

			expect(source).toContain('<script lang="ts" effect>');
			expect(source).toContain("const FollowHighlight = yield* MakeFollowHighlight");
		},
	);

	it("mounts the editor through SER's component scope rather than a hand-run fiber", async () => {
		const surface = await ReadSource(
			"modules/frontend/src/lib/components/editor/surface.svelte",
		);
		const route = await ReadSource(
			"modules/frontend/src/routes/components/editor-route.svelte",
		);
		const hooks = await ReadSource("modules/frontend/src/hooks.client.ts");

		expect(surface).toContain('<script lang="ts" effect>');
		expect(surface).toContain("const editor = yield* EditorService;");
		expect(surface).toContain("Effect.gen(function* ()");
		expect(surface).toContain("yield* Effect.never;");
		expect(surface).toContain("Effect.ensuring(editor.Detach)");
		expect(surface).not.toContain("onMount(");
		expect(surface).not.toMatch(/\bEffect\.runFork\b|\brunFork\b/);
		expect(route).toContain(
			"yield* ReconcileOpenPath(active_path, workspace_id, retained_file);",
		);
		expect(route).toContain(".pipe(Effect.forkScoped);");
		expect(route).toContain("open_requests.IsCurrent(generation)");
		expect(route).not.toContain("Queue.offerUnsafe");
		expect(hooks).toContain("BrowserHttpClient.layerFetch");
		expect(hooks).toContain("ClientRuntime.make(");
	});

	it("keeps shell recovery in SER effects without browser Promise execution", async () => {
		const layout = await ReadSource("modules/frontend/src/routes/+layout.svelte");
		const pairing = await ReadSource("modules/frontend/src/lib/runtime/pairing.ts");

		expect(layout).toContain("const http_client = yield* HttpClient.HttpClient;");
		expect(layout).not.toMatch(/\bonMount\s*\(|\bEffect\.tryPromise\b|\.then\s*\(/);
		expect(pairing).toContain("const client = yield* HttpClient.HttpClient;");
		expect(pairing).not.toMatch(/\basync\b|\bawait\b|\bEffect\.tryPromise\b/);
	});
});
