import raw_file_associations from "@artisan/data/file-icons/associations.json";
import { Schema } from "effect";

import svelte_icon from "$lib/assets/jetbrains-file-icons/dark/svelte.svg";
import text_icon from "$lib/assets/jetbrains-file-icons/dark/text.svg";
import ts_test_icon from "$lib/assets/jetbrains-file-icons/dark/ts-test.svg";
import typescript_icon from "$lib/assets/jetbrains-file-icons/dark/typescript.svg";

const FileAssociations = Schema.Record(Schema.NonEmptyString, Schema.Array(Schema.NonEmptyString));

const fallback_file_associations = {
	"typescript-test": [".test.ts", ".spec.ts"],
	typescript: [".ts"],
	svelte: [".svelte"],
} as const;

const file_icons: Readonly<Record<string, string>> = {
	"typescript-test": ts_test_icon,
	typescript: typescript_icon,
	svelte: svelte_icon,
};

const load_file_associations = (): ReadonlyArray<readonly [string, string]> => {
	try {
		const associations = Schema.decodeUnknownSync(FileAssociations)(raw_file_associations);

		return Object.entries(associations)
			.flatMap(([language, suffixes]) =>
				suffixes.map((suffix) => [language, suffix] as const),
			)
			.toSorted((left, right) => right[1].length - left[1].length);
	} catch {
		return Object.entries(fallback_file_associations)
			.flatMap(([language, suffixes]) =>
				suffixes.map((suffix) => [language, suffix] as const),
			)
			.toSorted((left, right) => right[1].length - left[1].length);
	}
};

const file_associations = load_file_associations();

export const resolve_file_icon = (path: string): string => {
	const filename = path.split(/[\\/]/).at(-1) ?? path;
	const normalized_filename = filename.toLowerCase();
	const language = file_associations.find(([, suffix]) =>
		normalized_filename.endsWith(suffix.toLowerCase()),
	)?.[0];

	return (language === undefined ? undefined : file_icons[language]) ?? text_icon;
};
