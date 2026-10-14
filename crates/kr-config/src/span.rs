//! Finds where a key path is written in a TOML document, for error labels.

use std::ops::Range;

use toml::Spanned;
use toml::de::{DeTable, DeValue};

/// Span of `path` in `doc`: the array element at `index` if given, else the value for
/// plain values, else the key for tables. Falls back to the deepest key that exists.
pub fn find(
    doc: &Spanned<DeTable<'_>>,
    path: &[&str],
    index: Option<usize>,
) -> Option<Range<usize>> {
    let mut table = doc.get_ref();
    let mut best = None;
    for (i, seg) in path.iter().enumerate() {
        let (key, value) = table.iter().find(|(k, _)| &**k.get_ref() == *seg)?;
        best = Some(key.span());
        let last = i + 1 == path.len();
        match value.get_ref() {
            DeValue::Table(inner) if !last => table = inner,
            DeValue::Array(items) if last => {
                return Some(match index.and_then(|n| items.get(n)) {
                    Some(item) => item.span(),
                    None => value.span(),
                });
            }
            DeValue::Table(_) => return best,
            _ if last => return Some(value.span()),
            _ => return best,
        }
    }
    best
}
