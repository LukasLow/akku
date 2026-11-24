//! Zero-dependency Unicode UAX #29 graphemes, text helpers, and Myers diff engine.
//!
//! `akku::text` provides:
//! - UAX #29 Extended Grapheme Cluster iteration ([`graphemes`], [`grapheme_indices`],
//!   [`grapheme_count`], [`truncate_graphemes`]).
//! - Word wrapping ([`wrap`], [`wrap_with`], [`fill`], [`fill_with`]).
//! - Word splitting, case conversions, and URL slugs ([`split_words`], [`to_snake_case`],
//!   [`to_kebab_case`], [`to_camel_case`], [`to_pascal_case`], [`slugify`]).
//! - Edit distance & similarity ([`levenshtein`], [`levenshtein_graphemes`],
//!   [`normalized_similarity`]).
//! - Canonical Unicode normalization ([`nfc`], [`nfd`], [`is_nfc`], [`is_nfd`],
//!   [`normalize`], [`NormalizationForm`]). Compatibility forms (`NFKC`/`NFKD`) are out of
//!   scope for Wave 2.
//! - Linear-space Myers diff, unified diff, and patch application in [`diff`]
//!   (also re-exported at `akku::diff`).
//!
//! # Quick start
//!
//! ```
//! use akku::text::{grapheme_count, slugify, to_snake_case, truncate_graphemes};
//!
//! assert_eq!(grapheme_count("👨‍👩‍👧‍👦🇩🇪"), 2);
//! assert_eq!(truncate_graphemes("👨‍👩‍👧‍👦🇩🇪!", 2, "…"), "👨‍👩‍👧‍👦…");
//! assert_eq!(to_snake_case("HTTPServerV2"), "http_server_v2");
//! assert_eq!(slugify("Crème Brûlée!"), "creme-brulee");
//! ```
//!
//! # Diffing
//!
//! ```
//! use akku::text::diff::{apply_patch, unified_diff};
//!
//! let old = "alpha\nbeta\ngamma\n";
//! let new = "alpha\nBETA\ngamma\n";
//! let patch = unified_diff(old, new, "a/file.txt", "b/file.txt", 1);
//! assert_eq!(apply_patch(old, &patch).unwrap(), new);
//! ```

mod case;
pub mod diff;
mod distance;
mod grapheme;
mod normalize;
mod tables;
mod wrap;

pub use self::case::{
    slugify, split_words, to_camel_case, to_kebab_case, to_pascal_case, to_snake_case,
};
pub use self::distance::{levenshtein, levenshtein_graphemes, normalized_similarity};
pub use self::grapheme::{
    grapheme_count, grapheme_indices, graphemes, truncate_graphemes, GraphemeIndices, Graphemes,
};
pub use self::normalize::{is_nfc, is_nfd, nfc, nfd, normalize, NormalizationForm};
pub use self::wrap::{fill, fill_with, wrap, wrap_with, WidthMode, WrapOptions};
