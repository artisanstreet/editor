import { createRequire } from "node:module";
import { resolve } from "node:path";

const require = createRequire(process.execPath);

let loaded_koffi: object | undefined;

/** Defers native loading until the SEA bootstrap has materialized its verified runtime. */
const LoadKoffi = () => {
	loaded_koffi ??= require(
		process.env.ARTISAN_NATIVE_RUNTIME === undefined
			? "koffi"
			: resolve(process.env.ARTISAN_NATIVE_RUNTIME, "koffi"),
	) as object;

	return loaded_koffi;
};

const koffi = new Proxy(
	{},
	{
		get: (_, property) => {
			const target = LoadKoffi();
			const value = Reflect.get(target, property, target) as unknown;

			return typeof value === "function" ? value.bind(target) : value;
		},
	},
);

export default koffi;
