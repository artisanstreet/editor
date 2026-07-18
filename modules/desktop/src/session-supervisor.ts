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
}

interface ActiveGeneration {
	readonly generation: number;
}

/** Owns one backend utility process and fences stale renderer connection generations. */
export class DesktopSessionSupervisor {
	readonly #options: DesktopSessionSupervisorOptions;
	#disposing = false;
	#generation = 0;
	#restart_attempt = 0;
	#utility: DesktopUtilityProcessShape | undefined;
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
		utility.on("exit", () => {
			if (this.#utility !== utility) {
				return;
			}

			this.#utility = undefined;
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

		const control = this.#options.create_channel();
		const stream = this.#options.create_channel();
		const generation = ++this.#generation;
		const previous = this.#active_by_web_contents.get(event.sender.id);

		if (previous) {
			utility.postMessage({ kind: "artisan:close-generation", generation: previous.generation });
		}

		this.#active_by_web_contents.set(event.sender.id, {
			generation,
		});
		utility.postMessage(
			{ generation, kind: "artisan:connect" },
			[control.port1, stream.port1],
		);

		if (event.sender.isDestroyed()) {
			throw new Error("Artisan renderer was destroyed before the connection was transferred");
		}

		event.sender.postMessage(
			"artisan:connection",
			{ generation, kind: "artisan:connection" },
			[control.port2, stream.port2],
		);

		return { generation };
	}

	Dispose() {
		this.#disposing = true;
		this.#close_active_generations();
		this.#utility?.kill();
		this.#utility = undefined;
	}

	#close_active_generations() {
		this.#active_by_web_contents.clear();
	}

	#restart() {
		if (this.#disposing) {
			return;
		}

		const milliseconds = Math.min(5_000, 100 * 2 ** Math.min(this.#restart_attempt++, 5));
		this.#options.schedule(() => this.Start(), milliseconds);
	}
}
