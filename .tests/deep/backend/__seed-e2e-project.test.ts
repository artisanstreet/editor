import { homedir } from "node:os";

import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import { make_backend_runtime, ThreadRetentionClock } from "@artisan/backend";
import { ProtocolServer } from "@artisan/backend";
import { RuntimeMetadata } from "../../../modules/backend/src/runtime/metadata";
import { make_transport_test_harness_with_protocol_server } from "../../transport/message-channel-harness";

const fixture_now = "2026-08-18T05:00:00.000Z";
const database_path = process.env["ARTISAN_E2E_SEED_DB"];
const project_name = process.env["ARTISAN_E2E_SEED_PROJECT"] ?? "artisan-e2e-demo";
const migrations_path = new URL(
	"../../../modules/backend/drizzle",
	import.meta.url,
).pathname.replace(/^\/([A-Za-z]:)/, "$1");

describe.skipIf(database_path === undefined)("seed e2e project", () => {
	it("attaches the demo project into the target database", async () => {
		let next_id = 0;
		const runtime = make_backend_runtime({
			database_path: database_path as string,
			migrations_path,
			retention_clock: Layer.succeed(ThreadRetentionClock, {
				Now: Effect.succeed(fixture_now),
			}),
			runtime_metadata: Layer.succeed(RuntimeMetadata, {
				instance_id: "seed_e2e_project",
				MakeId: (prefix) => Effect.sync(() => `${prefix}_${Date.now()}_${++next_id}`),
				Now: Effect.succeed(fixture_now),
			}),
		});
		const protocol_server = await runtime.runPromise(ProtocolServer);
		const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
		try {
			const roots = await Effect.runPromise(harness.client.ListProjectDirectories({}));
			const home = roots.directories.find((entry) => homedir().endsWith(entry.display_name));
			expect(home).toBeDefined();
			if (home === undefined) return;
			const children = await Effect.runPromise(
				harness.client.ListProjectDirectories({ parent_directory_id: home.directory_id }),
			);
			const demo = children.directories.find((entry) => entry.display_name === project_name);
			expect(demo).toBeDefined();
			if (demo === undefined) return;
			const project = await Effect.runPromise(
				harness.client.SelectProjectDirectory({ directory_id: demo.directory_id }),
			);
			expect(project.display_name).toBe(project_name);
		} finally {
			await harness.dispose();
			await runtime.dispose();
		}
	}, 60_000);
});
