import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import { make_backend_runtime, ProtocolServer, ThreadRetentionClock } from "@artisan/backend";
import { RuntimeMetadata } from "../../../modules/backend/src/runtime/metadata";
import { make_transport_test_harness_with_protocol_server } from "../../transport/message-channel-harness";

const migrations_path = fileURLToPath(new URL("../../../modules/backend/drizzle", import.meta.url));

const fixture_now = "2026-08-17T20:00:00.000Z";

/**
 * Live end-to-end proof of the machine switch chain: a real ArtisanClient
 * over the real ProtocolServer asks the real broker to start Forge inside an
 * actual WSL distribution, then completes the pairing exchange against the
 * endpoint the handoff produced. Machine-specific by nature (needs WSL, a
 * provisioned distribution, and `ARTISAN_WSL_AE_COMMAND` when the payload is
 * staged rather than installed), so it runs only when explicitly asked for:
 *
 *   ARTISAN_WSL_E2E=<distribution> pnpm run test:focus .tests/deep/backend/wsl-machine-switch.test.ts
 */
const distribution = process.env["ARTISAN_WSL_E2E"];

describe.skipIf(distribution === undefined || process.platform !== "win32")(
	"WSL machine switch (live)",
	() => {
		it("connects, starts the in-distro Forge, and pairs against its endpoint", async () => {
			const directory = await mkdtemp(join(tmpdir(), "artisan-wsl-machine-switch-"));
			let next_id = 0;
			const runtime = make_backend_runtime({
				database_path: join(directory, "artisan.db"),
				migrations_path,
				retention_clock: Layer.succeed(ThreadRetentionClock, {
					Now: Effect.succeed(fixture_now),
				}),
				runtime_metadata: Layer.succeed(RuntimeMetadata, {
					instance_id: "wsl_machine_switch_test",
					MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
					Now: Effect.succeed(fixture_now),
				}),
			});
			const protocol_server = await runtime.runPromise(ProtocolServer);
			const harness = await make_transport_test_harness_with_protocol_server(protocol_server);
			try {
				const machines = await Effect.runPromise(harness.client.GetHostMachines);
				expect(machines.machines[0]).toMatchObject({ id: "local", kind: "local" });
				expect(machines.machines.map((machine) => machine.id)).toContain(
					`wsl:${distribution}`,
				);

				const outcome = await Effect.runPromise(
					harness.client.ConnectHostMachine({ machine_id: `wsl:${distribution}` }),
				);
				expect(outcome.status).toBe("connected");
				if (outcome.status !== "connected") return;
				expect(outcome.endpoint).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/$/);
				expect(outcome.pair_code.length).toBeGreaterThan(0);

				const paired = await fetch(`${outcome.endpoint}api/pair`, {
					body: JSON.stringify({ code: outcome.pair_code }),
					headers: { "content-type": "application/json" },
					method: "POST",
				});
				expect(paired.ok).toBe(true);
				expect(paired.headers.get("set-cookie")).toContain("artisan_forge_session=");
			} finally {
				await harness.dispose();
				await runtime.dispose();
			}
		}, 180_000);
	},
);
