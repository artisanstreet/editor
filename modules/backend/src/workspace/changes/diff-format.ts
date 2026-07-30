import { FILE_HEADERS_ONLY, formatPatch, parsePatch, type StructuredPatch } from "diff";
import { Option } from "effect";

const ParsePatch = Option.liftThrowable(parsePatch);
const FormatPatch = Option.liftThrowable((patch: StructuredPatch) =>
	formatPatch(patch, FILE_HEADERS_ONLY),
);

/** Validates one canonical patch against jsdiff's decoded file headers. */
export function workspace_diff_patch_matches_path(patch_text: string, path: string) {
	return ParsePatch(patch_text).pipe(
		Option.filter((patches) => patches.length === 1),
		Option.flatMap((patches) => Option.fromNullishOr(patches[0])),
		Option.filter(
			(patch) => patch.oldFileName === `a/${path}` && patch.newFileName === `b/${path}`,
		),
		Option.flatMap((patch) =>
			FormatPatch(patch).pipe(
				Option.filter((canonical) => canonical === patch_text),
				Option.map(() => true),
			),
		),
		Option.getOrElse(() => false),
	);
}
