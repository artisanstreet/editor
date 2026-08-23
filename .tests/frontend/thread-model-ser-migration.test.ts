import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const ReadComponent = (path: string) =>
	readFileSync(resolve("modules/frontend/src/routes/components", path), "utf8");
const ReadBrowser = (path: string) =>
	readFileSync(resolve("modules/frontend/src/lib/browser", path), "utf8");

describe("thread and model SER migration", () => {
	it("routes policy changes through an Effect-valued controller callback", () => {
		const route = ReadComponent("thread-route.svelte");
		const workspace = ReadComponent("thread-workspace.svelte");
		const composer = ReadComponent("thread-composer.svelte");
		const selector = ReadComponent("model-selector/view.svelte");

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

	it("keeps lifecycle ingress in scoped Effect subscriptions without unsafe queue writes", () => {
		for (const path of [
			"thread-composer.svelte",
			"thread-route.svelte",
			"model-selector/view.svelte",
			"model-selector/engine-section.svelte",
		]) {
			const source = ReadComponent(path);
			expect(source).not.toContain("Queue.offerUnsafe");
		}
		const route = ReadComponent("thread-route.svelte");
		expect(route).toContain("RunConversationSubscription(");
		expect(route).toContain("RunAuthoritativeSubscription(");
		expect(route).toContain("Effect.forkIn(");
		const workspace = ReadComponent("thread-workspace.svelte");
		expect(workspace).toContain("Queue.unbounded<void>()");
		expect(workspace).toContain("yield* Queue.take(anchor_layout_wake)");
		expect(workspace).toContain("Queue.offerUnsafe(anchor_layout_wake, undefined)");
		expect(workspace).toContain("const RequestAnchorLayout =");
		expect(workspace).toContain("const RequestAnchorLayoutUnsafe =");
		expect(workspace).not.toContain("anchor_layout_requests");
		expect(workspace).not.toContain("Queue.unbounded<AnchorLayoutRequest>()");
		expect(route).not.toContain("onDestroy(");
	});

	it("keeps image visibility and first-submission completion inside yielded generators", () => {
		const route = ReadComponent("thread-route.svelte");
		const workspace = ReadComponent("thread-workspace.svelte");

		expect(route).toContain("const UpdateImageAttachmentVisibility =");
		expect(route).toContain("yield* RequestImageAttachment(attachment)");
		expect(workspace).toContain(") => Effect.Effect<void>");
		expect(
			route.indexOf("yield* SendMessage(claimed.submission, claimed.command_id)"),
		).toBeLessThan(route.indexOf("yield* claimed.Complete"));
	});

	it("coalesces interaction refreshes behind one route-owned deferred", () => {
		const route = ReadComponent("thread-route.svelte");

		expect(route).toContain("const interaction_refresh_in_flight = yield* Ref.make<");
		expect(route).toContain("const claimed = yield* Ref.modify(interaction_refresh_in_flight");
		expect(route).toContain(
			"Effect.forkIn(CompleteInteractionRefresh(deferred), thread_scope)",
		);
		/**
		 * Overlapping requests still share one read, but a request that arrives
		 * mid-read is owed its own: the open read observed the world before it
		 * asked, and every caller here asks because something just changed.
		 */
		expect(route).toContain("interaction_refresh_queued");
		expect(route).toContain("conversation_resync_queued");
		expect(route).not.toContain(
			"if (claimed !== deferred) return yield* Deferred.await(claimed);",
		);
		expect(route).toContain("[Resync, RefreshInteractionContext],");
		expect(route).toContain('{ concurrency: "unbounded", discard: true }');
		expect(route).toContain("yield* run_usage_lease.Select(next?.run_id)");
		expect(route).toContain("client.SubscribeThreadWork(thread_id)");
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
