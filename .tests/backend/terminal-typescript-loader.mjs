/** Resolves extensionless repository imports when Node executes a TypeScript crash fixture. */
export async function resolve(specifier, context, next_resolve) {
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
