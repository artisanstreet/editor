import { describe, expect, it } from "vitest";

import { DesktopSessionSupervisor } from "@artisan/desktop";

function make_port() {
	return { close: () => undefined };
}

describe("desktop session supervisor", () => {
	it("transfers atomic pairs and fences the previous renderer generation", () => {
		const utility_messages: Array<{ readonly message: unknown; readonly ports: number }> = [];
		const utility = {
			kill: () => true,
			on: () => undefined,
			postMessage: (message: unknown, ports: ReadonlyArray<object> = []) =>
				utility_messages.push({ message, ports: ports.length }),
		};
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => ({ port1: make_port(), port2: make_port() }),
			fork_utility: () => utility,
			paths: {
				database_path: "database",
				frontend_index_path: "frontend",
				frontend_root: "frontend-root",
				migrations_path: "migrations",
				preload_path: "preload",
				utility_path: "utility",
			},
			schedule: () => undefined,
		});
		const sent: Array<{ readonly message: unknown; readonly ports: number }> = [];
		const event = {
			sender: {
				id: 7,
				isDestroyed: () => false,
				postMessage: (_channel: string, message: unknown, ports: ReadonlyArray<object>) =>
					sent.push({ message, ports: ports.length }),
			},
		};

		expect(supervisor.RequestConnection(event)).toEqual({ generation: 1 });
		expect(supervisor.RequestConnection(event)).toEqual({ generation: 2 });
		expect(utility_messages).toMatchObject([
			{ message: { generation: 1, kind: "artisan:connect" }, ports: 2 },
			{ message: { generation: 1, kind: "artisan:close-generation" } },
			{ message: { generation: 2, kind: "artisan:connect" }, ports: 2 },
		]);
		expect(sent).toEqual([
			{ message: { generation: 1, kind: "artisan:connection" }, ports: 2 },
			{ message: { generation: 2, kind: "artisan:connection" }, ports: 2 },
		]);
	});
});
