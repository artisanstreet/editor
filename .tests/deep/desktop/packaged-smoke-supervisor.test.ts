import { describe, expect, it } from "vitest";

import { DesktopSessionSupervisor } from "@artisan/desktop";

const paths = {
	database_path: "database",
	frontend_index_path: "frontend",
	frontend_root: "frontend-root",
	migrations_path: "migrations",
	preload_path: "preload",
	utility_path: "utility",
};

describe("packaged desktop smoke supervision", () => {
	it("records accepted termination and a distinct replacement utility before replay", async () => {
		const scheduled: Array<{ readonly callback: () => void; readonly milliseconds: number }> =
			[];
		const listeners: Array<Map<string, (message?: unknown) => void>> = [];
		let kills = 0;
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => ({
				port1: { close: () => undefined },
				port2: { close: () => undefined },
			}),
			fork_utility: () => {
				const callbacks = new Map<string, (message?: unknown) => void>();
				listeners.push(callbacks);
				return {
					kill: () => {
						kills += 1;
						return true;
					},
					on: (event: unknown, listener: unknown) =>
						callbacks.set(String(event), listener as (message?: unknown) => void),
					pid: listeners.length,
					postMessage: () => undefined,
				};
			},
			paths,
			schedule: (callback, milliseconds) => scheduled.push({ callback, milliseconds }),
		});

		supervisor.Start();
		const restarted = supervisor.ForceRestartForSmoke();
		listeners[0]?.get("exit")?.();
		scheduled.find(({ milliseconds }) => milliseconds === 100)?.callback();

		await expect(restarted).resolves.toEqual({
			kill_accepted: true,
			next_utility_epoch: 2,
			next_utility_pid: 2,
			previous_utility_exit_observed: true,
			previous_utility_epoch: 1,
			previous_utility_pid: 1,
		});
		expect(kills).toBe(1);
	});

	it("waits for native-load evidence from the replacement utility epoch", async () => {
		const scheduled: Array<() => void> = [];
		const listeners: Array<Map<string, (message?: unknown) => void>> = [];
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => ({
				port1: { close: () => undefined },
				port2: { close: () => undefined },
			}),
			fork_utility: () => {
				const callbacks = new Map<string, (message?: unknown) => void>();
				listeners.push(callbacks);
				return {
					kill: () => true,
					on: (event: unknown, listener: unknown) =>
						callbacks.set(String(event), listener as (message?: unknown) => void),
					pid: listeners.length,
					postMessage: () => undefined,
				};
			},
			paths,
			schedule: (callback) => scheduled.push(callback),
		});

		supervisor.Start();
		listeners[0]?.get("message")?.({
			bounded_native_binding_path: "first.node",
			koffi_native_binding_path: "first-koffi.node",
			kind: "artisan:smoke-native-load",
			native_store_root: "first-store",
			node_pty_module_path: "first/node-pty",
		});
		await expect(supervisor.AwaitSmokeNativeLoad(1)).resolves.toMatchObject({
			utility_epoch: 1,
			utility_pid: 1,
		});

		const restarted = supervisor.ForceRestartForSmoke();
		listeners[0]?.get("exit")?.();
		scheduled[1]?.();
		const restart = await restarted;
		const replacement_native = supervisor.AwaitSmokeNativeLoad(restart.next_utility_epoch);
		let settled = false;
		void replacement_native.then(() => (settled = true));
		await Promise.resolve();
		expect(settled).toBe(false);

		listeners[1]?.get("message")?.({
			bounded_native_binding_path: "second.node",
			koffi_native_binding_path: "second-koffi.node",
			kind: "artisan:smoke-native-load",
			native_store_root: "second-store",
			node_pty_module_path: "second/node-pty",
		});
		await expect(replacement_native).resolves.toMatchObject({
			bounded_native_binding_path: "second.node",
			koffi_native_binding_path: "second-koffi.node",
			utility_epoch: 2,
			utility_pid: 2,
		});
	});

	it("does not leave disposal pending when a disconnected utility rejects shutdown", async () => {
		let killed = 0;
		const supervisor = new DesktopSessionSupervisor({
			create_channel: () => ({
				port1: { close: () => undefined },
				port2: { close: () => undefined },
			}),
			fork_utility: () => ({
				kill: () => {
					killed += 1;
					return true;
				},
				on: () => undefined,
				postMessage: () => {
					throw new Error("disconnected");
				},
			}),
			paths,
			schedule: () => undefined,
		});

		supervisor.Start();
		await expect(supervisor.Dispose()).resolves.toBeUndefined();
		expect(killed).toBe(1);
	});
});
