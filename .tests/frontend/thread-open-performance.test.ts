import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const route_path = join(
	process.cwd(),
	"modules/frontend/src/routes/components/thread-route.svelte",
);

describe("thread-open performance boundaries", () => {
	it("opens the route from one authoritative thread-open snapshot and resumes its cursor", () => {
		const source = readFileSync(route_path, "utf8");
		const startup = source.slice(0, source.indexOf("const ReplaceSnapshot"));

		expect(startup.match(/client\.GetThreadOpen\(route_id\)/g)).toHaveLength(1);
		for (const rpc of [
			"client.ListThreads",
			"client.GetThreadSession",
			"client.GetThreadWork",
			"client.GetConversation",
		]) {
			expect(startup).not.toContain(rpc);
		}
		expect(source).toContain("client.SubscribeConversation(thread_id, {");
		expect(source).toContain("conversation_id,");
		expect(source).toContain("last_patch_sequence: snapshot.last_patch_sequence,");
	});

	it("keeps generic event recovery to interaction context and observes accepted local state", () => {
		const source = readFileSync(route_path, "utf8");
		const events = source.slice(source.indexOf("RunAuthoritativeSubscription("));

		expect(events).toContain("RefreshInteractionContext");
		expect(events).not.toContain("Resync");
		expect(source).toContain("const CurrentSnapshot = Effect.gen(function* () {");
		expect(source).toContain("AwaitAcceptedProjection(\n\t\t\t\t\t\tCurrentSnapshot,");
		expect(source).toContain("ThreadRouteId(thread_id) !== route_id");
	});
});
