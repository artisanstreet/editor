//! Pure file-icon resolution for native consumers.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/conversation/file-icon.ts`. It resolves only a
//! path and returns a stable semantic [`FileIcon`] key. A native renderer maps
//! that key to the corresponding vendored asset; resolution never loads an
//! asset and never returns a Svelte URL.
//!
//! The checked-in `modules/data/file-icons/associations.json` currently
//! decodes to the same associations as [`FILE_ASSOCIATIONS`]. The table is
//! kept statically in the already-sorted order used by the TypeScript
//! `toSorted` call, so lookup remains deterministic and performs no I/O. The
//! same table is also the fallback for the reached association set.

#![allow(clippy::module_name_repetitions)]

/// Semantic icon key returned for a file path.
///
/// The key is independent of a renderer or asset URL. Native rendering maps
/// each variant to its stable asset-catalog entry:
///
/// | Key | Asset |
/// | --- | --- |
/// | [`Self::Text`] | `jetbrains.text` |
/// | [`Self::TypeScriptTest`] | `jetbrains.ts-test` |
/// | [`Self::TypeScript`] | `jetbrains.typescript` |
/// | [`Self::Svelte`] | `jetbrains.svelte` |
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FileIcon {
    /// Generic text-file icon used for unknown or unmatched paths.
    #[default]
    Text,
    /// TypeScript test icon for `.test.ts` and `.spec.ts` files.
    TypeScriptTest,
    /// TypeScript icon for `.ts` files that do not match a longer suffix.
    TypeScript,
    /// Svelte icon for `.svelte` files.
    Svelte,
}

impl FileIcon {
    /// Every semantic key in stable enum order.
    pub const ALL: [Self; 4] = [
        Self::Text,
        Self::TypeScriptTest,
        Self::TypeScript,
        Self::Svelte,
    ];

    /// Returns the short stable semantic key used by native renderers.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::TypeScriptTest => "typescript-test",
            Self::TypeScript => "typescript",
            Self::Svelte => "svelte",
        }
    }

    /// Returns the corresponding asset-catalog identifier.
    ///
    /// This is an identifier only; calling it does not load the asset. A
    /// native renderer owns the mapping from this identifier to its packaged
    /// SVG asset.
    #[must_use]
    pub const fn asset_id(self) -> &'static str {
        match self {
            Self::Text => "jetbrains.text",
            Self::TypeScriptTest => "jetbrains.ts-test",
            Self::TypeScript => "jetbrains.typescript",
            Self::Svelte => "jetbrains.svelte",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileAssociation {
    suffix: &'static str,
    icon: FileIcon,
}

// This is the stable descending-suffix-length result of the checked-in JSON:
// `typescript-test` contributes the two eight-byte suffixes first, followed
// by `.svelte`, then the shorter `.ts` fallback. Equal-length entries retain
// their JSON order, as JavaScript's `toSorted` does.
const FALLBACK_FILE_ASSOCIATIONS: &[FileAssociation] = &[
    FileAssociation {
        suffix: ".test.ts",
        icon: FileIcon::TypeScriptTest,
    },
    FileAssociation {
        suffix: ".spec.ts",
        icon: FileIcon::TypeScriptTest,
    },
    FileAssociation {
        suffix: ".svelte",
        icon: FileIcon::Svelte,
    },
    FileAssociation {
        suffix: ".ts",
        icon: FileIcon::TypeScript,
    },
];

// Keep a named lookup view separate from the fallback declaration so the
// source makes the no-I/O association policy explicit.
const FILE_ASSOCIATIONS: &[FileAssociation] = FALLBACK_FILE_ASSOCIATIONS;

/// Resolves the semantic icon for `path`.
///
/// Basename extraction treats both `/` and `\\` as separators, preserving the
/// TypeScript behavior for leading, repeated, and trailing separators. The
/// basename is matched case-insensitively against the statically ordered
/// suffixes; therefore a longer suffix such as `.test.ts` wins over `.ts`.
/// Unknown paths, paths without a matching suffix, and an empty basename use
/// [`FileIcon::Text`].
///
/// This function is pure and performs no filesystem, asset, or association
/// loading at lookup time.
#[must_use]
pub fn resolve_file_icon(path: &str) -> FileIcon {
    let filename = path
        .rsplit(|character| character == '/' || character == '\\')
        .next()
        .unwrap_or(path);

    FILE_ASSOCIATIONS
        .iter()
        .find(|association| has_case_insensitive_suffix(filename, association.suffix))
        .map_or(FileIcon::Text, |association| association.icon)
}

fn has_case_insensitive_suffix(value: &str, suffix: &str) -> bool {
    value.len() >= suffix.len()
        && value.as_bytes()[value.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}
