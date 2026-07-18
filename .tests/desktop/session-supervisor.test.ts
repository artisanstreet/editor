import { describe, expect, it } from "vitest";

import { DesktopSessionSupervisor } from "@artisan/desktop";

function make_port() {
	return { close: () => undefined };
}

function make_tracked_channel() {
	const ports = [0, 1].map(() => ({
		closes: 0,
		close() {
			this.closes += 1;
		},
	}));

	return { channel: { port1: ports[0]!, port2: ports[1]! }, ports };
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

	it("closes every still-owned port when utility transfer rejects", () => {
		const first = make_tracked_channel();
		const second = make_tracked_channel();
		const channels = [first.channel, second.channel];
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => channels.shift()!,
			fork_utility: () => ({
				kill: () => true,
				on: () => undefined,
				postMessage: () => {
					throw new Error("utility unavailable");
				},
			}),
			paths: {
				database_path: "database",
				frontend_index_path: "frontend",
				frontend_root: "root",
				migrations_path: "migrations",
				preload_path: "preload",
				utility_path: "utility",
			},
			schedule: () => undefined,
		});

		expect(() =>
			supervisor.RequestConnection({
				sender: { id: 1, isDestroyed: () => false, postMessage: () => undefined },
			}),
		).toThrow("utility unavailable");
		expect([...first.ports, ...second.ports].map(({ closes }) => closes)).toEqual([1, 1, 1, 1]);
	});

	it("fences the utility and cleans renderer-owned ports when renderer delivery rejects", () => {
		const first = make_tracked_channel();
		const second = make_tracked_channel();
		const channels = [first.channel, second.channel];
		const messages: unknown[] = [];
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => channels.shift()!,
			fork_utility: () => ({
				kill: () => true,
				on: () => undefined,
				postMessage: (message: unknown) => messages.push(message),
			}),
			paths: {
				database_path: "database",
				frontend_index_path: "frontend",
				frontend_root: "root",
				migrations_path: "migrations",
				preload_path: "preload",
				utility_path: "utility",
			},
			schedule: () => undefined,
		});

		expect(() =>
			supervisor.RequestConnection({
				sender: {
					id: 1,
					isDestroyed: () => false,
					postMessage: () => {
						throw new Error("renderer gone");
					},
				},
			}),
		).toThrow("renderer gone");
		expect(messages).toEqual([
			{ generation: 1, kind: "artisan:connect" },
			{ generation: 1, kind: "artisan:close-generation" },
		]);
		expect(first.ports[1]!.closes).toBe(1);
		expect(second.ports[1]!.closes).toBe(1);
	});

	it("fences a previous generation before allocating a replacement pair", () => {
		let channel_creations = 0;
		const messages: unknown[] = [];
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => {
				channel_creations += 1;

				return { port1: make_port(), port2: make_port() };
			},
			fork_utility: () => ({
				kill: () => true,
				on: () => undefined,
				postMessage: (message: unknown) => {
					messages.push(message);
					if (
						(message as { readonly kind?: unknown }).kind === "artisan:close-generation"
					) {
						throw new Error("utility fence failed");
					}
				},
			}),
			paths: {
				database_path: "database",
				frontend_index_path: "frontend",
				frontend_root: "root",
				migrations_path: "migrations",
				preload_path: "preload",
				utility_path: "utility",
			},
			schedule: () => undefined,
		});
		const event = { sender: { id: 1, isDestroyed: () => false, postMessage: () => undefined } };

		supervisor.RequestConnection(event);
		expect(() => supervisor.RequestConnection(event)).toThrow("utility fence failed");
		expect(channel_creations).toBe(2);
		expect(messages).toContainEqual({ generation: 1, kind: "artisan:close-generation" });
	});

	it("waits for utility shutdown acknowledgement before the bounded kill fallback", async () => {
		const callbacks = new Map<string, (value?: unknown) => void>();
		const scheduled: Array<{ readonly callback: () => void; readonly milliseconds: number }> =
			[];
		let killed = 0;
		let shutdown_requested = 0;
		const utility = {
			kill: () => {
				killed += 1;
				return true;
			},
			on: (event: unknown, listener: unknown) => {
				callbacks.set(String(event), listener as (value?: unknown) => void);
			},
			postMessage: (message: unknown) => {
				if ((message as { readonly kind?: unknown }).kind === "artisan:shutdown") {
					shutdown_requested += 1;
				}
			},
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
			schedule: (callback, milliseconds) => scheduled.push({ callback, milliseconds }),
			shutdown_deadline_ms: 10,
		});

		supervisor.Start();
		const dispose = supervisor.Dispose();

		expect(shutdown_requested).toBe(1);
		expect(killed).toBe(0);
		callbacks.get("message")?.({ kind: "artisan:shutdown-complete" });
		await dispose;
		expect(killed).toBe(0);
		expect(scheduled.some(({ milliseconds }) => milliseconds === 10)).toBe(true);
	});

	it("treats a utility exit during shutdown as an immediate acknowledgement", async () => {
		const callbacks = new Map<string, (value?: unknown) => void>();
		const utility = {
			kill: () => true,
			on: (event: unknown, listener: unknown) =>
				callbacks.set(String(event), listener as (value?: unknown) => void),
			postMessage: () => undefined,
		};
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => ({ port1: make_port(), port2: make_port() }),
			fork_utility: () => utility,
			paths: {
				database_path: "database",
				frontend_index_path: "frontend",
				frontend_root: "root",
				migrations_path: "migrations",
				preload_path: "preload",
				utility_path: "utility",
			},
			schedule: () => undefined,
		});

		supervisor.Start();
		const dispose = supervisor.Dispose();
		callbacks.get("exit")?.();
		await expect(dispose).resolves.toBeUndefined();
	});

	it("uses exponential crash backoff and only resets after a stable run", () => {
		const callbacks = new Map<string, (value?: unknown) => void>();
		const scheduled: Array<{ readonly callback: () => void; readonly milliseconds: number }> =
			[];
		const utilities: Array<typeof callbacks> = [];
		const MakeUtility = () => {
			const listeners = new Map<string, (value?: unknown) => void>();
			utilities.push(listeners);

			return {
				kill: () => true,
				on: (event: unknown, listener: unknown) =>
					listeners.set(String(event), listener as (value?: unknown) => void),
				postMessage: () => undefined,
			};
		};
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => ({ port1: make_port(), port2: make_port() }),
			fork_utility: MakeUtility,
			paths: {
				database_path: "database",
				frontend_index_path: "frontend",
				frontend_root: "frontend-root",
				migrations_path: "migrations",
				preload_path: "preload",
				utility_path: "utility",
			},
			schedule: (callback, milliseconds) => scheduled.push({ callback, milliseconds }),
			stable_run_ms: 1_000,
		});

		supervisor.Start();
		utilities[0]?.get("exit")?.();
		const restart = scheduled.find(({ milliseconds }) => milliseconds === 100);
		restart?.callback();
		utilities[1]?.get("exit")?.();
		expect(scheduled.map(({ milliseconds }) => milliseconds)).toContain(100);
		expect(scheduled.map(({ milliseconds }) => milliseconds)).toContain(200);
		scheduled.filter(({ milliseconds }) => milliseconds === 1_000)[1]?.callback();
		scheduled.find(({ milliseconds }) => milliseconds === 200)?.callback();
		utilities[2]?.get("exit")?.();
		expect(scheduled.map(({ milliseconds }) => milliseconds)).toContain(100);
	});
});
