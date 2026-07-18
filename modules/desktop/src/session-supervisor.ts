import type {
	DesktopIpcEventShape,
	DesktopMessageChannelShape,
	DesktopPaths,
	DesktopUtilityProcessShape,
} from "./contracts";

export interface DesktopSessionSupervisorOptions {
	readonly create_channel: () => DesktopMessageChannelShape;
	readonly fork_utility: (utility_path: string) => DesktopUtilityProcessShape;
	readonly paths: DesktopPaths;
	readonly schedule: (callback: () => void, milliseconds: number) => unknown;
	readonly shutdown_deadline_ms?: number;
	readonly stable_run_ms?: number;
}

interface ActiveGeneration {
	readonly generation: number;
}

/** Owns one backend utility process and fences stale renderer connection generations. */
export class DesktopSessionSupervisor {
	readonly #options: DesktopSessionSupervisorOptions;
	#disposing = false;
	#dispose_promise: Promise<void> | undefined;
	#generation = 0;
	#restart_attempt = 0;
	#utility: DesktopUtilityProcessShape | undefined;
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
		utility.on("message", (message: unknown) => {
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

	Dispose(): Promise<void> {
		if (this.#dispose_promise) {
			return this.#dispose_promise;
		}

		this.#disposing = true;
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
					utility.kill();
					finish();
				}
			}, this.#options.shutdown_deadline_ms ?? 5_000);
			utility.postMessage({ kind: "artisan:shutdown" });
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
