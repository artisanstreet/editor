import { createRequire } from "node:module";
import { resolve } from "node:path";

const require = createRequire(process.execPath);
type NodePtyModule = {
	readonly spawn: (file: string, args: ReadonlyArray<string>, options: object) => unknown;
};

let node_pty: NodePtyModule | undefined;

/** Defers native loading until the SEA bootstrap has materialized its verified runtime. */
const LoadNodePty = () => {
	node_pty ??= require(
		process.env.ARTISAN_NATIVE_RUNTIME === undefined
			? "node-pty"
			: resolve(process.env.ARTISAN_NATIVE_RUNTIME, "node-pty"),
	) as NodePtyModule;

	return node_pty;
};

export const spawn: NodePtyModule["spawn"] = (...args) => LoadNodePty().spawn(...args);
