//! The coverage allow list: items the gate must not report, each with a reason.
//!
//! Two sources feed one list: in-code entries (`Coverage::allow(&[("Model.unused", "why")])`)
//! and an optional TOML file (`Coverage::allow_file("tests/coverage_allow.toml")`):
//!
//! ```toml
//! [[allow]]
//! key = "Model.unused"
//! reason = "driven by the OS tray, no window path"
//!
//! [[allow]]
//! key = "dialog.slint|Confirm|*"   # a trailing `*` makes the key a prefix
//! reason = "covered by the app's own tests"
//! ```
//!
//! A key is a callback (`Model.name`) or an element key (`file|Component|id`,
//! `file|Component|"label"` or `file|Component|<Type@line>`, see
//! [`super::inventory::ElementItem::key`]). A key ending in `*` matches every inventory key that
//! starts with the text before it.
//!
//! An entry that matches nothing in the inventory is STALE (the UI changed and the entry stayed);
//! the gate reports it so the list cannot rot. A missing or unparsable file is one `coverage`
//! finding (made by the caller); the gate still runs with the in-code entries.

use serde::Deserialize;
use std::{collections::BTreeSet, fs, path::Path};

/// The TOML file: a list of `[[allow]]` tables.
#[derive(Debug, Deserialize)]
struct AllowFile {
    #[serde(default)]
    allow: Vec<AllowEntry>,
}

/// One `[[allow]]` table.
#[derive(Debug, Deserialize)]
struct AllowEntry {
    key: String,
    reason: String,
}

/// Whether the allow key `entry` (a trailing `*` makes it a prefix) matches `key`.
fn matches(entry: &str, key: &str) -> bool {
    entry
        .strip_suffix('*')
        .map_or(entry == key, |prefix| key.starts_with(prefix))
}

/// In-code and file allow entries, in that order.
#[derive(Debug, Default)]
pub struct AllowList {
    entries: Vec<(String, String)>,
}

impl AllowList {
    /// A list of the given in-code `(key, reason)` entries.
    pub(crate) fn new(entries: &[(String, String)]) -> Self {
        Self {
            entries: entries.to_vec(),
        }
    }

    /// The in-code entries plus the entries of the TOML file at `file`, when given. The second
    /// element is the error text for the finding when the file cannot be read or parsed; the
    /// list then holds the in-code entries only.
    pub(crate) fn load(
        entries: &[(String, String)],
        file: Option<&Path>,
    ) -> (Self, Option<String>) {
        let mut list = Self::new(entries);
        let Some(file) = file else {
            return (list, None);
        };
        let text = match fs::read_to_string(file) {
            Ok(text) => text,
            Err(err) => return (list, Some(format!("cannot read allow file: {err}"))),
        };
        match toml::from_str::<AllowFile>(&text) {
            Ok(parsed) => {
                list.entries
                    .extend(parsed.allow.into_iter().map(|e| (e.key, e.reason)));
                (list, None)
            }
            Err(err) => (list, Some(format!("cannot parse allow file: {err}"))),
        }
    }

    /// The reason `key` is on the list, when it is (an entry ending in `*` is a prefix).
    pub(crate) fn reason(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find_map(|(entry, reason)| matches(entry, key).then_some(reason.as_str()))
    }

    /// The `(entry, reason)` pairs that match none of `keys` (the inventory's callback and
    /// element keys): allow entries the UI no longer needs.
    pub(crate) fn stale(&self, keys: &BTreeSet<String>) -> Vec<(String, String)> {
        self.entries
            .iter()
            .filter(|(entry, _)| {
                let hit = entry.strip_suffix('*').map_or_else(
                    || keys.contains(entry),
                    |prefix| keys.iter().any(|k| k.starts_with(prefix)),
                );
                !hit
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::AllowList;
    use std::collections::BTreeSet;

    fn pair(key: &str, reason: &str) -> (String, String) {
        (key.to_owned(), reason.to_owned())
    }

    #[test]
    fn exact_and_prefix_entries_match_and_unmatched_ones_are_stale() {
        let list = AllowList::new(&[
            pair("Model.unused", "exact"),
            pair("dialog.slint|Confirm|*", "prefix"),
            pair("Model.gone", "stale"),
            pair("nothing.slint|*", "stale prefix"),
        ]);
        assert_eq!(list.reason("Model.unused"), Some("exact"));
        assert_eq!(list.reason("Model.unused2"), None);
        assert_eq!(list.reason("dialog.slint|Confirm|ok"), Some("prefix"));
        assert_eq!(list.reason("dialog.slint|Other|ok"), None);
        let keys: BTreeSet<String> = [
            "Model.unused".to_owned(),
            "dialog.slint|Confirm|ok".to_owned(),
        ]
        .into();
        let stale = list.stale(&keys);
        assert_eq!(
            stale,
            [
                pair("Model.gone", "stale"),
                pair("nothing.slint|*", "stale prefix")
            ]
        );
    }

    #[test]
    fn a_missing_file_keeps_the_in_code_entries_and_reports_the_error() {
        let (list, error) = AllowList::load(
            &[pair("Model.unused", "kept")],
            Some(std::path::Path::new("definitely/not/here.toml")),
        );
        assert_eq!(list.reason("Model.unused"), Some("kept"));
        assert!(error.is_some_and(|e| e.contains("cannot read allow file")));
    }
}
