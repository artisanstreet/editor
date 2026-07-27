/** Resolves extensionless repository imports for an isolated TypeScript crash fixture. */
type ResolveResult = { format?: string; shortCircuit?: boolean; url: string };
type ResolveNext = (specifier: string, context: object) => Promise<ResolveResult>;

export async function resolve(specifier: string, context: object, next_resolve: ResolveNext) {
	try {
		return await next_resolve(specifier, context);
	} catch (error) {
		const is_missing_module =
			typeof error === "object" &&
			error !== null &&
			"code" in error &&
			error.code === "ERR_MODULE_NOT_FOUND";
		const is_relative = specifier.startsWith(".") || specifier.startsWith("file:");

		if (!is_missing_module || !is_relative) {
			throw error;
		}

		return next_resolve(`${specifier}.ts`, context);
	}
}
