import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import { make_backend_runtime, ProtocolServer, ThreadRetentionClock } from "@artisan/backend";
import { RuntimeMetadata } from "../../../modules/backend/src/runtime/metadata";
import { make_transport_test_harness_with_protocol_server } from "../../transport/message-channel-harness";

const fixture_now = "2026-08-18T08:00:00.000Z";
const database_path = process.env["ARTISAN_E2E_SEED_DB"];
const migrations_path = fileURLToPath(new URL("../../../modules/backend/drizzle", import.meta.url));

describe.skipIf(database_path === undefined)("seed e2e threads", () => {
	it("creates rail fixture threads in the target database", async () => {
		let next_id = 0;
		const runtime = make_backend_runtime({
			database_path: database_path as string,
			migrations_path,
			retention_clock: Layer.succeed(ThreadRetentionClock, {
				Now: Effect.succeed(fixture_now),
			}),
			runtime_metadata: Layer.succeed(RuntimeMetadata, {
				instance_id: "seed_e2e_threads",
				MakeId: (prefix) => Effect.sync(() => `${prefix}_${Date.now()}_${++next_id}`),
				Now: Effect.succeed(fixture_now),
			}),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const catalog = await Effect.runPromise(harness.client.ListProjects);
			const project = catalog.projects[0];
			for (const title of [
				"Heap-20260818T102032.heapsnapshot It has been idling without any work for a while growing — inspect it and tell me whether the growth is content or a leak",
				"Ship the WSL machine selector end to end",
				"Rework the thread hover card contents",
			]) {
				const created = await Effect.runPromise(
					harness.client.CreateThread({
						title,
						...(project === undefined ? {} : { project_id: project.project_id }),
					}),
				);
				expect(created.title).toBe(title);
			}
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	}, 60_000);
});
