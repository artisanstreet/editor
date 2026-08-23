import { Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DefaultPathSeparator,
	DefaultTimeFormat,
	FormatLocalDateTime,
	FormatPathSeparators,
	ResolveDisplayFormatPreferences,
} from "../../modules/frontend/src/lib/appearance/display-format";
import { AppearanceState } from "../../modules/frontend/src/lib/runtime/appearance-preferences";

describe("display formatting preferences", () => {
	it("uses the host-native path separator until a preference is stored", () => {
		expect(DefaultPathSeparator("Windows")).toBe("backslash");
		expect(DefaultPathSeparator("Win32")).toBe("backslash");
		expect(DefaultPathSeparator("macOS")).toBe("forward-slash");
		expect(DefaultPathSeparator("Linux")).toBe("forward-slash");
	});

	it("uses the locale clock until a preference is stored", () => {
		expect(DefaultTimeFormat("en-US")).toBe("12-hour");
		expect(DefaultTimeFormat("en-GB")).toBe("24-hour");
	});

	it("changes path presentation without changing its segments", () => {
		expect(FormatPathSeparators("src/lib/file.ts", "backslash")).toBe("src\\lib\\file.ts");
		expect(FormatPathSeparators("C:\\work\\file.ts", "forward-slash")).toBe("C:/work/file.ts");
	});

	it("keeps explicit stored choices independent of host defaults", () => {
		const stored = Schema.decodeUnknownSync(AppearanceState)({
			path_separator: "forward-slash",
			shader_enabled: true,
			time_format: "24-hour",
			version: 1,
		});

		expect(ResolveDisplayFormatPreferences(stored)).toEqual({
			path_separator: "forward-slash",
			time_format: "24-hour",
		});
	});

	it("makes the selected clock explicit", () => {
		const options = {
			hour: "numeric",
			minute: "2-digit",
			timeZone: "UTC",
		} as const;
		const twelve_hour = FormatLocalDateTime(
			"2026-08-21T21:05:00.000Z",
			"12-hour",
			options,
			"en-US",
		);
		const twenty_four_hour = FormatLocalDateTime(
			"2026-08-21T21:05:00.000Z",
			"24-hour",
			options,
			"en-US",
		);

		expect(twelve_hour).toMatch(/9:05\sPM/u);
		expect(twenty_four_hour).toBe("21:05");
	});
});
