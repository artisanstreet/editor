/** Resolves extensionless repository imports when Node executes a TypeScript crash fixture. */
type ResolveResult = { format?: string; shortCircuit?: boolean; url: string };
type ResolveNext = (specifier: string, context: object) => Promise<ResolveResult>;

export async function resolve(specifier: string, context: object, next_resolve: ResolveNext) {
	try {
		return await next_resolve(specifier, context);
	} catch (error) {
		const error_code =
			typeof error === "object" &&
			error !== null &&
			"code" in error &&
			typeof error.code === "string"
				? error.code
				: undefined;
		const is_relative = specifier.startsWith(".") || specifier.startsWith("file:");

		if (!is_relative) {
			throw error;
		}

		if (error_code === "ERR_MODULE_NOT_FOUND") return next_resolve(`${specifier}.ts`, context);
		if (error_code === "ERR_UNSUPPORTED_DIR_IMPORT") {
			try {
				return await next_resolve(`${specifier}.ts`, context);
			} catch (file_error) {
				const file_error_code =
					typeof file_error === "object" &&
					file_error !== null &&
					"code" in file_error &&
					typeof file_error.code === "string"
						? file_error.code
						: undefined;
				if (file_error_code === "ERR_MODULE_NOT_FOUND")
					return next_resolve(`${specifier}/index.ts`, context);
				throw file_error;
			}
		}

		throw error;
	}
}
