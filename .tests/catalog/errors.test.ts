import { describe, expect, it } from "vitest";
import { Schema } from "effect";

import {
	ArtisanErrorCode,
	artisan_error_codes,
	artisan_error_definitions,
	lookup_artisan_error,
} from "@artisan/catalog";

describe("Artisan error catalog", () => {
	it("mints every named code with a valid shape, uniquely, and with a definition", () => {
		const decode = Schema.decodeUnknownOption(ArtisanErrorCode);
		const codes = Object.values(artisan_error_codes);

		for (const code of codes) {
			expect(decode(code)._tag).toBe("Some");
			expect(lookup_artisan_error(code).code).toBe(code);
		}
		expect(new Set(codes).size).toBe(codes.length);
		expect(artisan_error_definitions).toHaveLength(codes.length);
	});

	it("degrades an unrecognized code to the unknown definition without losing the code", () => {
		const future = "AE-FUTURE-999";

		expect(lookup_artisan_error(future)).toMatchObject({
			code: future,
			title: "Unexpected engine failure",
		});
		expect(lookup_artisan_error("not-a-code")).toMatchObject({
			code: artisan_error_codes.unknown,
		});
	});
});
