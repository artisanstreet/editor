/** Structural Electron surface kept injectable for deterministic shell tests. */
export interface DesktopPortShape {
	readonly close: () => void;
}

export interface DesktopMessageChannelShape {
	readonly port1: DesktopPortShape;
	readonly port2: DesktopPortShape;
}

export interface DesktopUtilityProcessShape {
	readonly kill: () => boolean;
	readonly on: (...args: ReadonlyArray<unknown>) => unknown;
	readonly postMessage: (message: unknown, transfer?: Array<DesktopPortShape>) => void;
}

export interface DesktopWebContentsShape {
	readonly id: number;
	readonly isDestroyed: () => boolean;
	readonly postMessage: (
		channel: string,
		message: unknown,
		transfer: Array<DesktopPortShape>,
	) => void;
}

export interface DesktopIpcEventShape {
	readonly sender: DesktopWebContentsShape;
}

export interface DesktopPaths {
	readonly database_path: string;
	readonly frontend_index_path: string;
	readonly frontend_root: string;
	readonly migrations_path: string;
	readonly preload_path: string;
	readonly utility_path: string;
}
