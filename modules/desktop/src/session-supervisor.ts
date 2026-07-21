import type {
	DesktopIpcEventShape,
	DesktopMessageChannelShape,
	DesktopPaths,
	DesktopSmokeConnection,
	DesktopUtilityProcessShape,
} from "./contracts";

export interface DesktopSessionSupervisorOptions {
	readonly create_channel: () => DesktopMessageChannelShape;
	readonly fork_utility: (utility_path: string) => DesktopUtilityProcessShape;
	readonly paths: DesktopPaths;
	readonly report_diagnostic?: (message: unknown) => void;
	readonly schedule: (callback: () => void, milliseconds: number) => unknown;
	readonly shutdown_deadline_ms?: number;
	readonly stable_run_ms?: number;
}

interface ActiveGeneration {
	readonly generation: number;
}

export interface DesktopSmokeRestartEvidence {
	readonly kill_accepted: true;
	readonly next_utility_epoch: number;
	readonly next_utility_pid: number | undefined;
	readonly previous_utility_exit_observed: true;
	readonly previous_utility_epoch: number;
	readonly previous_utility_pid: number | undefined;
}

export interface DesktopSmokeNativeLoadEvidence {
	readonly bounded_native_binding_path: string;
	readonly koffi_native_binding_path: string;
	readonly native_store_root: string;
	readonly node_pty_module_path: string;
	readonly utility_epoch: number;
	readonly utility_pid: number | undefined;
}

interface PendingSmokeRestart {
	readonly previous_utility: DesktopUtilityProcessShape;
	readonly previous_utility_epoch: number;
	readonly resolve: (evidence: DesktopSmokeRestartEvidence) => void;
	readonly reject: (cause: Error) => void;
}

/** Owns one backend utility process and fences stale renderer connection generations. */
export class DesktopSessionSupervisor {
	readonly #options: DesktopSessionSupervisorOptions;
	#disposing = false;
	#dispose_promise: Promise<void> | undefined;
	#generation = 0;
	#restart_attempt = 0;
	#utility: DesktopUtilityProcessShape | undefined;
	#utility_epoch = 0;
	#pending_smoke_restart: PendingSmokeRestart | undefined;
	#smoke_native_load: DesktopSmokeNativeLoadEvidence | undefined;
	#smoke_native_load_waiter:
		| {
				readonly minimum_utility_epoch: number;
				readonly resolve: (evidence: DesktopSmokeNativeLoadEvidence) => void;
		  }
		| undefined;
	#shutdown_ack: (() => void) | undefined;
	readonly #active_by_web_contents = new Map<number, ActiveGeneration>();

	constructor(options: DesktopSessionSupervisorOptions) {
		this.#options = options;
	}

	Start() {
		if (this.#utility) {
			return;
		}

		const utility = this.#options.fork_utility(this.#options.paths.utility_path);

		this.#utility = utility;
		const utility_epoch = ++this.#utility_epoch;
		const pending_smoke_restart = this.#pending_smoke_restart;
		if (pending_smoke_restart && pending_smoke_restart.previous_utility !== utility) {
			this.#pending_smoke_restart = undefined;
			pending_smoke_restart.resolve({
				kill_accepted: true,
				next_utility_epoch: utility_epoch,
				next_utility_pid: utility.pid,
				previous_utility_exit_observed: true,
				previous_utility_epoch: pending_smoke_restart.previous_utility_epoch,
				previous_utility_pid: pending_smoke_restart.previous_utility.pid,
			});
		}
		utility.on("message", (message: unknown) => {
			if (
				typeof message === "object" &&
				message !== null &&
				typeof (message as { readonly kind?: unknown }).kind === "string" &&
				(message as { readonly kind: string }).kind.startsWith("artisan:utility-")
			) {
				this.#options.report_diagnostic?.(message);
			}
			if (
				typeof message === "object" &&
				message !== null &&
				(message as { readonly kind?: unknown }).kind === "artisan:smoke-native-load"
			) {
				const candidate = message as Partial<DesktopSmokeNativeLoadEvidence>;
				if (
					typeof candidate.bounded_native_binding_path === "string" &&
					typeof candidate.koffi_native_binding_path === "string" &&
					typeof candidate.native_store_root === "string" &&
					typeof candidate.node_pty_module_path === "string"
				) {
					this.#smoke_native_load = {
						bounded_native_binding_path: candidate.bounded_native_binding_path,
						koffi_native_binding_path: candidate.koffi_native_binding_path,
						native_store_root: candidate.native_store_root,
						node_pty_module_path: candidate.node_pty_module_path,
						utility_epoch,
						utility_pid: utility.pid,
					};
					if (
						this.#smoke_native_load_waiter !== undefined &&
						utility_epoch >= this.#smoke_native_load_waiter.minimum_utility_epoch
					) {
						this.#smoke_native_load_waiter.resolve(this.#smoke_native_load);
						this.#smoke_native_load_waiter = undefined;
					}
				}
			}
			if (
				typeof message === "object" &&
				message !== null &&
				(message as { readonly kind?: unknown }).kind === "artisan:shutdown-complete"
			) {
				this.#shutdown_ack?.();
			}
		});
		this.#options.schedule(() => {
			if (this.#utility === utility && !this.#disposing) {
				this.#restart_attempt = 0;
			}
		}, this.#options.stable_run_ms ?? 30_000);
		utility.on("exit", () => {
			if (this.#utility !== utility) {
				return;
			}

			this.#utility = undefined;
			this.#shutdown_ack?.();
			this.#close_active_generations();
			this.#restart();
		});
	}

	RequestConnection(event: DesktopIpcEventShape) {
		this.Start();
		const utility = this.#utility;

		if (!utility) {
			throw new Error("Artisan backend utility is unavailable");
		}
		if (event.sender.isDestroyed()) {
			throw new Error("Artisan renderer was destroyed before a connection could be created");
		}

		const previous = this.#active_by_web_contents.get(event.sender.id);

		if (previous) {
			utility.postMessage({
				kind: "artisan:close-generation",
				generation: previous.generation,
			});
		}

		const control = this.#options.create_channel();
		const stream = this.#options.create_channel();
		const generation = ++this.#generation;

		try {
			utility.postMessage({ generation, kind: "artisan:connect" }, [
				control.port1,
				stream.port1,
			]);
		} catch (cause) {
			this.#close_ports([control.port1, control.port2, stream.port1, stream.port2]);
			throw cause;
		}

		this.#active_by_web_contents.set(event.sender.id, { generation });

		try {
			event.sender.postMessage(
				"artisan:connection",
				{ generation, kind: "artisan:connection" },
				[control.port2, stream.port2],
			);
		} catch (cause) {
			this.#active_by_web_contents.delete(event.sender.id);
			this.#close_ports([control.port2, stream.port2]);
			try {
				utility.postMessage({ kind: "artisan:close-generation", generation });
			} catch {
				/** The utility exit handler will fence any remaining owned generation. */
			}
			throw cause;
		}

		return { generation };
	}

	/** Opens a main-process-only pair for the explicit packaged release smoke. */
	RequestSmokeConnection(): DesktopSmokeConnection {
		this.Start();
		const utility = this.#utility;
		if (!utility) throw new Error("Artisan backend utility is unavailable");
		const control = this.#options.create_channel();
		const stream = this.#options.create_channel();
		const generation = ++this.#generation;
		try {
			utility.postMessage({ generation, kind: "artisan:connect" }, [
				control.port1,
				stream.port1,
			]);
		} catch (cause) {
			this.#close_ports([control.port1, control.port2, stream.port1, stream.port2]);
			throw cause;
		}
		return { control_port: control.port2, generation, stream_port: stream.port2 };
	}

	/** Returns the native bindings that the smoke utility actually resolved and opened. */
	AwaitSmokeNativeLoad(minimum_utility_epoch = 1): Promise<DesktopSmokeNativeLoadEvidence> {
		if (
			this.#smoke_native_load &&
			this.#smoke_native_load.utility_epoch >= minimum_utility_epoch
		) {
			return Promise.resolve(this.#smoke_native_load);
		}
		if (this.#smoke_native_load_waiter) {
			return Promise.reject(
				new Error("Packaged smoke native-load observation is already pending"),
			);
		}
		return new Promise((resolve) => {
			this.#smoke_native_load_waiter = { minimum_utility_epoch, resolve };
		});
	}

	/** Kills one observed utility and resolves only after a distinct replacement starts. */
	ForceRestartForSmoke(): Promise<DesktopSmokeRestartEvidence> {
		this.Start();
		const utility = this.#utility;
		if (!utility) {
			return Promise.reject(
				new Error("Artisan backend utility is unavailable for smoke restart"),
			);
		}
		if (this.#pending_smoke_restart) {
			return Promise.reject(new Error("A packaged smoke restart is already pending"));
		}

		return new Promise<DesktopSmokeRestartEvidence>((resolve, reject) => {
			this.#pending_smoke_restart = {
				previous_utility: utility,
				previous_utility_epoch: this.#utility_epoch,
				reject,
				resolve,
			};
			let kill_accepted = false;
			try {
				kill_accepted = utility.kill();
			} catch (cause) {
				this.#pending_smoke_restart = undefined;
				reject(
					cause instanceof Error
						? cause
						: new Error("Packaged smoke utility kill failed"),
				);
				return;
			}
			if (!kill_accepted) {
				this.#pending_smoke_restart = undefined;
				reject(new Error("Packaged smoke utility kill was rejected"));
			}
		});
	}

	Dispose(): Promise<void> {
		if (this.#dispose_promise) {
			return this.#dispose_promise;
		}

		this.#disposing = true;
		this.#pending_smoke_restart?.reject(
			new Error("Desktop supervisor disposed during smoke restart"),
		);
		this.#pending_smoke_restart = undefined;
		this.#smoke_native_load_waiter = undefined;
		this.#close_active_generations();
		const utility = this.#utility;

		if (!utility) {
			return Promise.resolve();
		}

		this.#dispose_promise = new Promise<void>((resolve) => {
			let settled = false;
			const finish = () => {
				if (settled) {
					return;
				}

				settled = true;
				this.#shutdown_ack = undefined;
				this.#utility = undefined;
				resolve();
			};

			this.#shutdown_ack = finish;
			this.#options.schedule(() => {
				if (!settled) {
					try {
						utility.kill();
					} catch {
						/** Disposal is complete even if the already-dead utility rejects termination. */
					} finally {
						finish();
					}
				}
			}, this.#options.shutdown_deadline_ms ?? 5_000);
			try {
				utility.postMessage({ kind: "artisan:shutdown" });
			} catch {
				/** A disconnected utility cannot acknowledge; terminate it and complete disposal. */
				try {
					utility.kill();
				} finally {
					finish();
				}
			}
		});

		return this.#dispose_promise;
	}

	#close_active_generations() {
		this.#active_by_web_contents.clear();
	}

	#close_ports(ports: ReadonlyArray<{ readonly close: () => void }>) {
		for (const port of ports) {
			try {
				port.close();
			} catch {
				/** Best-effort cleanup after a failed ownership transfer. */
			}
		}
	}

	#restart() {
		if (this.#disposing) {
			return;
		}

		const milliseconds = Math.min(5_000, 100 * 2 ** Math.min(this.#restart_attempt++, 5));
		this.#options.schedule(() => this.Start(), milliseconds);
	}
}
