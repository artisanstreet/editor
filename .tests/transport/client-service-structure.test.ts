import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source_root = join(process.cwd(), "modules", "transport", "src", "internal");

describe("transport client source structure", () => {
	it("keeps the public client facade distinct from its implementation directory", () => {
		expect(existsSync(join(source_root, "..", "client.ts"))).toBe(true);
		expect(existsSync(join(source_root, "..", "client"))).toBe(false);
		expect(existsSync(join(source_root, "..", "client-api"))).toBe(true);
	});

	it("keeps the client composer and operation families bounded", () => {
		const client_source = readFileSync(join(source_root, "client-service.ts"), "utf8");
		const client_lines = client_source.split(/\r?\n/u).length;
		expect(client_lines).toBeLessThan(900);
		expect(client_source).not.toMatch(
			/make_client_subscription_coordinator\s*\(\s*options\.event_capacity/u,
		);

		for (const entry of readdirSync(join(source_root, "api"), {
			withFileTypes: true,
		})) {
			if (!entry.isFile() || !entry.name.endsWith(".ts")) {
				continue;
			}
			const lines = readFileSync(join(source_root, "api", entry.name), "utf8").split(
				/\r?\n/u,
			).length;
			expect(lines, entry.name).toBeLessThan(500);
		}
	});

	it("keeps subscription composition and lifecycle modules bounded", () => {
		const subscriptions_root = join(source_root, "subscriptions");
		const coordinator_source = readFileSync(join(subscriptions_root, "coordinator.ts"), "utf8");
		const coordinator_lines = readFileSync(
			join(subscriptions_root, "coordinator.ts"),
			"utf8",
		).split(/\r?\n/u).length;

		expect(coordinator_lines).toBeLessThan(500);
		expect(coordinator_source).not.toMatch(
			/make_client_subscription_coordinator\s*=\s*\(\s*event_capacity/u,
		);
		expect(coordinator_source).toContain("yield* SubscriptionOptions");
		expect(coordinator_source).toContain("yield* SubscriptionIdentity");
		expect(coordinator_source).toContain("yield* SubscriptionProtocol");
		expect(coordinator_source).toContain("yield* SubscriptionErrorReporter");

		for (const entry of readdirSync(subscriptions_root, {
			withFileTypes: true,
		})) {
			if (!entry.isFile() || !entry.name.endsWith(".ts")) {
				continue;
			}

			const source = readFileSync(join(subscriptions_root, entry.name), "utf8");
			const lines = source.split(/\r?\n/u).length;

			expect(lines, entry.name).toBeLessThan(700);
			expect(source, entry.name).not.toContain("as unknown as");
		}
	});
});
