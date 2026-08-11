import { cpSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Data, NtExecutable, NtExecutableResource, Resource } from "resedit";
import { describe, expect, it } from "vitest";

import { BrandForgeExecutable } from "../../.config/forge.rolldown.config.ts";

const root = new URL("../..", import.meta.url);

describe("Forge executable branding", () => {
	it("injects the dedicated Artisan Forge process icon", () => {
		const temporary_root = mkdtempSync(join(tmpdir(), "artisan-forge-branding-"));
		const executable_path = join(temporary_root, "Artisan Forge.exe");
		try {
			cpSync(process.execPath, executable_path);
			BrandForgeExecutable(executable_path, "0.2.2");

			const executable = NtExecutable.from(readFileSync(executable_path), {
				ignoreCert: true,
			});
			const resources = NtExecutableResource.from(executable);
			const branded_icon = Resource.IconGroupEntry.fromEntries(
				resources.entries,
			)[0]?.getIconItemsFromEntries(resources.entries)[0];
			const expected_icon = Data.IconFile.from(
				readFileSync(
					new URL("modules/frontend/src/lib/assets/barekey/artisan-forge-icon.ico", root),
				),
			).icons[0]?.data;

			expect(branded_icon?.isRaw()).toBe(true);
			expect(expected_icon?.isRaw()).toBe(true);
			if (!branded_icon?.isRaw() || !expected_icon?.isRaw()) return;
			expect(Buffer.from(branded_icon.bin)).toEqual(Buffer.from(expected_icon.bin));
		} finally {
			rmSync(temporary_root, { force: true, recursive: true });
		}
	});
});
