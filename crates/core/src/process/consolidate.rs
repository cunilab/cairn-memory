use super::Origin;
use std::collections::BTreeSet;

/// Cosine at or above this: the newer memory supersedes the older. The cohort (same owner, scope,
/// project and kind) is enforced by `Store::consolidate_memory`'s candidate query.
pub const SUPERSEDE_AT: f32 = 0.92;
/// Cosine in `[RELATE_AT, SUPERSEDE_AT)`: add an automatic `relates` link.
pub const RELATE_AT: f32 = 0.75;
/// Neighbours considered per memory.
pub const CANDIDATES: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    Supersede,
    Relate,
}

/// Anything lower, or a non-finite value, is left alone.
pub fn band(cosine: f32) -> Option<Band> {
    if !cosine.is_finite() || cosine > 1.0 + 1e-5 {
        None
    } else if cosine >= SUPERSEDE_AT {
        Some(Band::Supersede)
    } else if cosine >= RELATE_AT {
        Some(Band::Relate)
    } else {
        None
    }
}

/// One side of a near-duplicate pair.
#[derive(Clone, Copy, Debug)]
pub struct Side<'a> {
    pub branch: Option<&'a str>,
    pub origin: Origin,
}

/// The band actually applied (CM-SPEC-008 §4.2). A supersede becomes a plain `relates` when the two
/// memories belong to different branches (branch-specific knowledge), when an `agent` memory would
/// be replaced by a `system` one, or when their rare tokens differ.
pub fn guard(band: Band, newer: Side, older: Side, newer_text: &str, older_text: &str) -> Band {
    if band != Band::Supersede {
        return band;
    }
    let other_branch = matches!((newer.branch, older.branch), (Some(a), Some(b)) if a != b);
    let system_over_agent = newer.origin == Origin::System && older.origin == Origin::Agent;
    if other_branch || system_over_agent || rare_tokens_differ(newer_text, older_text) {
        Band::Relate
    } else {
        Band::Supersede
    }
}

/// Words from a title and summary that carry a version, number, path, file name, or backticks.
/// True when those sets are not the same, so near-copies that name different things stay linked.
fn rare_tokens_differ(a: &str, b: &str) -> bool {
    rare_tokens(a) != rare_tokens(b)
}

fn rare_tokens(text: &str) -> BTreeSet<&str> {
    text.split_whitespace().filter_map(rare_word).collect()
}

fn rare_word(word: &str) -> Option<&str> {
    // Backticks stay: a wrapped word is rare even with no digit, slash, or file suffix.
    let word = word.trim_matches(|c: char| c.is_ascii_punctuation() && c != '`');
    if word.is_empty() {
        return None;
    }
    let inner = word
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .unwrap_or(word);
    let wrapped = inner.len() != word.len();
    if wrapped
        || inner.chars().any(|c| c.is_ascii_digit())
        || inner.contains('/')
        || has_file_suffix(inner)
    {
        Some(inner)
    } else {
        None
    }
}

/// A `.ext` file suffix: a dot, then a letter, then only letters and digits.
fn has_file_suffix(word: &str) -> bool {
    let Some((stem, ext)) = word.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && ext.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_follow_the_thresholds() {
        assert_eq!(band(0.99), Some(Band::Supersede));
        assert_eq!(band(0.92), Some(Band::Supersede));
        assert_eq!(band(0.919), Some(Band::Relate));
        assert_eq!(band(0.75), Some(Band::Relate));
        assert_eq!(band(0.749), None);
        assert_eq!(band(f32::NAN), None);
    }

    #[test]
    fn consolidate_keeps_branch_specific() {
        let on = |branch, origin| Side { branch, origin };
        let main = on(Some("main"), Origin::Agent);
        assert_eq!(guard(Band::Supersede, main, main, "", ""), Band::Supersede);
        assert_eq!(
            guard(Band::Supersede, main, on(None, Origin::Agent), "", ""),
            Band::Supersede
        );
        assert_eq!(
            guard(
                Band::Supersede,
                main,
                on(Some("feat/x"), Origin::Agent),
                "",
                ""
            ),
            Band::Relate
        );
        // an agent memory is never replaced by a system one; the reverse is fine
        assert_eq!(
            guard(Band::Supersede, on(None, Origin::System), main, "", ""),
            Band::Relate
        );
        assert_eq!(
            guard(Band::Supersede, main, on(None, Origin::System), "", ""),
            Band::Supersede
        );
        assert_eq!(
            guard(Band::Relate, on(None, Origin::System), main, "", ""),
            Band::Relate
        );
    }

    #[test]
    fn rare_tokens_veto_supersede() {
        let side = Side {
            branch: None,
            origin: Origin::Agent,
        };
        assert_eq!(
            guard(
                Band::Supersede,
                side,
                side,
                "Use node 20 for the desktop app",
                "Use node 22 for the desktop app",
            ),
            Band::Relate
        );
        assert_eq!(
            guard(
                Band::Supersede,
                side,
                side,
                "Read src/app.ts before editing",
                "Read src/app.tsx before editing",
            ),
            Band::Relate
        );
    }

    #[test]
    fn same_tokens_still_supersede() {
        let side = Side {
            branch: None,
            origin: Origin::Agent,
        };
        assert_eq!(
            guard(
                Band::Supersede,
                side,
                side,
                "Use node 20 for the desktop app",
                "The desktop app should use node 20",
            ),
            Band::Supersede
        );
        assert_eq!(
            guard(
                Band::Supersede,
                side,
                side,
                "See `config.toml` for the listen port",
                "The listen port is in `config.toml`",
            ),
            Band::Supersede
        );
        // A trailing period is not a different token from the same number.
        assert_eq!(
            guard(
                Band::Supersede,
                side,
                side,
                "Use node 20.",
                "use node 20 now",
            ),
            Band::Supersede
        );
    }
}
