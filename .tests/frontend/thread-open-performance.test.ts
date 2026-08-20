import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const route_path = join(
	process.cwd(),
	"modules/frontend/src/routes/components/thread-route.svelte",
);
const gate_path = join(
	process.cwd(),
	"modules/frontend/src/routes/components/thread-route-gate.svelte",
);
const controller_path = join(
	process.cwd(),
	"modules/frontend/src/lib/thread-interaction/thread-open-controller.ts",
);
const store_path = join(process.cwd(), "modules/frontend/src/lib/conversation/store.ts");

describe("thread-open performance boundaries", () => {
	it("opens the route from one authoritative thread-open snapshot and resumes its cursor", () => {
		const source = readFileSync(route_path, "utf8");
		const gate = readFileSync(gate_path, "utf8");
		const controller = readFileSync(controller_path, "utf8");
		const startup = source.slice(0, source.indexOf("const ReplaceSnapshot"));

		expect(startup).not.toContain("client.GetThreadOpen");
		expect(startup).toContain("thread_open");
		expect(gate).toContain("yield* thread_opens.Current(route_id)");
		expect(gate).toContain("Load.pipe(Effect.forkScoped)");
		expect(gate).toContain('aria-label="Loading thread"');
		expect(controller.match(/client\.GetThreadOpen\(key\)/g)).toHaveLength(1);
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
		expect(source).toContain("const AwaitCanonicalUserMessage = (command_id: string) =>");
		expect(source).not.toContain("AwaitAcceptedProjection");
		expect(source).not.toContain("CurrentSnapshot");
		expect(source).toContain("ThreadRouteId(thread_id) !== route_id");
	});

	it("uses pre-indexed participant groups without rebuilding a filtered snapshot", () => {
		const source = readFileSync(store_path, "utf8");
		const participant = source.slice(
			source.indexOf("export const MakeParticipantConversationRenderWindow"),
		);

		expect(participant).toContain("group_ids_by_participant_agent_id");
		expect(participant).toContain("root_group_ids");
		expect(participant).not.toContain("MakeConversationViewState");
		expect(participant).not.toContain("snapshot.items.filter");
		expect(participant).not.toContain("snapshot.turns.filter");
	});
});
