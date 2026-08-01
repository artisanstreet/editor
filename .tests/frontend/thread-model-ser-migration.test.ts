import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const ReadComponent = (path: string) =>
	readFileSync(resolve("modules/frontend/src/routes/components", path), "utf8");
const ReadBrowser = (path: string) =>
	readFileSync(resolve("modules/frontend/src/lib/browser", path), "utf8");

describe("thread and model SER migration", () => {
	it("routes policy changes through an Effect-valued controller callback", () => {
		const route = ReadComponent("thread-route.sv");
		const workspace = ReadComponent("thread-workspace.sv");
		const composer = ReadComponent("thread-composer.sv");
		const selector = ReadComponent("model-selector/view.sv");

		for (const source of [workspace, composer, selector]) {
			expect(source).toContain("onpolicychange?: (");
			expect(source).toContain("Effect.Effect<ThreadSessionPolicy");
		}
		expect(route).toContain("yield* UpdateSessionPolicy(policy)");
		expect(selector).toContain("return yield* persist(desired)");
		expect(selector).toContain("const policy_controller = yield* MakeModelPolicyController");
		expect(selector).toContain("yield* policy_controller.RequestRepair");
		expect(selector).not.toContain("onpolicychange(next);");
	});

	it("keeps synchronous lifecycle ingress behind yielded queue workers", () => {
		for (const path of [
			"thread-composer.sv",
			"thread-route.sv",
			"model-selector/view.sv",
			"model-selector/engine-section.sv",
		]) {
			const source = ReadComponent(path);
			expect(source).not.toContain("Queue.offerUnsafe");
			expect(source).not.toContain("Effect.sync");
			expect(source).not.toContain("Effect.flatMap");
			expect(source).not.toContain("Effect.andThen");
		}
		const workspace = ReadComponent("thread-workspace.sv");
		expect(workspace).toContain("yield* Queue.take(anchor_layout_requests)");
		expect(workspace).toContain("Queue.offerUnsafe(anchor_layout_requests");
		expect(workspace).not.toContain("Effect.sync");
		expect(workspace).not.toContain("Effect.flatMap");
		expect(workspace).not.toContain("Effect.andThen");
		expect(ReadComponent("thread-route.sv")).not.toContain("onDestroy(");
	});

	it("keeps image visibility and first-submission completion inside yielded generators", () => {
		const route = ReadComponent("thread-route.sv");
		const workspace = ReadComponent("thread-workspace.sv");

		expect(route).toContain("const UpdateImageAttachmentVisibility =");
		expect(route).toContain("yield* RequestImageAttachment(attachment)");
		expect(workspace).toContain(") => Effect.Effect<void>");
		expect(
			route.indexOf("yield* SendMessage(claimed.submission, claimed.command_id)"),
		).toBeLessThan(route.indexOf("yield* claimed.Complete"));
	});

	it("commits only the latest interaction refresh and waits for a competing draft claim", () => {
		const route = ReadComponent("thread-route.sv");

		expect(route).toContain(
			'import { MakeLatestRequestGate } from "$lib/lifecycle/latest-request-gate"',
		);
		expect(route).toContain(
			"const interaction_refresh_requests = yield* MakeLatestRequestGate",
		);
		expect(route).toContain("const interaction_refresh_commit = yield* Semaphore.make(1)");
		expect(route).toContain(
			"if (!(yield* interaction_refresh_requests.IsCurrent(generation))) return;",
		);
		expect(route).not.toContain("interaction_refresh_generation");
		expect(route).toContain("yield* run_usage_lease.Select(work?.run_id)");
		expect(route).toContain("AwaitPendingSubmissionClaim(thread_id)");
		expect(route).toContain('if (update.type === "snapshot")');
		expect(route).not.toContain('update.type === "snapshot"\n\t\t\t? ReplaceSnapshot');
		expect(route).toContain("const ReleaseImageAttachment = (attachment_id: string) =>");
		expect(route).toContain('from "$lib/browser/object-url"');
		expect(route).toContain("yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore)");
		expect(route).toContain(
			"const source = yield* CreateBrowserObjectUrl(bytes, result.value.media_type)",
		);
		expect(route).not.toContain("BrowserObjectUrlFailure");
		expect(route).not.toContain("yield* Effect.void;\n\t\t\t\t\treturn candidate;");
	});

	it("keeps object URL creation and revocation as tagged yielded browser Effects", () => {
		const object_url = ReadBrowser("object-url.ts");

		expect(object_url).toContain('Data.TaggedError("BrowserObjectUrlFailure")');
		expect(object_url).toContain("export const CreateBrowserObjectUrl");
		expect(object_url).toContain("export const ReleaseBrowserObjectUrl");
		expect(object_url).toContain("URL.createObjectURL");
		expect(object_url).toContain("URL.revokeObjectURL");
		expect(object_url).not.toContain("Effect.sync");
	});
});
