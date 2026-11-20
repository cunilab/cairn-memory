use crate::capture::WorkContext;
use crate::memory::{Entity, EntityKind};

pub const ENTITY_CAP: usize = 32;

/// Deterministic entities: the request's lists, plus repo-relative paths and backticked
/// identifiers found in the memory text. Deduplicated, capped at 32.
pub fn derive(texts: &[&str], context: &WorkContext) -> Vec<Entity> {
    let mut out: Vec<Entity> = Vec::new();
    let lists = [
        (EntityKind::File, &context.files),
        (EntityKind::Symbol, &context.symbols),
        (EntityKind::Command, &context.commands),
        (EntityKind::Test, &context.tests),
        (EntityKind::Service, &context.services),
    ];
    for (kind, names) in lists {
        for name in names {
            let name = if kind == EntityKind::File {
                normalize_path(name)
            } else {
                name.trim().to_owned()
            };
            push(&mut out, kind, name);
        }
    }
    for text in texts {
        for (index, part) in text.split('`').enumerate() {
            if index % 2 == 1 {
                push_code(&mut out, part.trim());
            } else {
                for word in part.split_whitespace() {
                    let word = word.trim_matches(|c: char| ",;:()[]\"'".contains(c) || c == '.');
                    if looks_like_path(word) {
                        push(&mut out, EntityKind::File, normalize_path(word));
                    }
                }
            }
        }
    }
    out.truncate(ENTITY_CAP);
    out
}

fn push(out: &mut Vec<Entity>, kind: EntityKind, name: String) {
    if name.is_empty() || name.chars().count() > 200 {
        return;
    }
    let name = if kind == EntityKind::Term {
        name.to_lowercase()
    } else {
        name
    };
    if !out.iter().any(|e| e.kind == kind && e.name == name) {
        out.push(Entity { kind, name });
    }
}

fn push_code(out: &mut Vec<Entity>, code: &str) {
    if code.is_empty() {
        return;
    }
    if looks_like_path(code) {
        push(out, EntityKind::File, normalize_path(code));
    } else if code.contains(char::is_whitespace) {
        push(out, EntityKind::Command, code.to_owned());
    } else if code
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'))
    {
        let kind =
            if code.contains("::") || code.contains('_') || code.chars().any(char::is_uppercase) {
                EntityKind::Symbol
            } else {
                EntityKind::Term
            };
        push(out, kind, code.to_owned());
    }
}

fn normalize_path(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_owned()
}

/// A slash path whose last segment has a short extension, such as `crates/core/src/lib.rs`.
fn looks_like_path(word: &str) -> bool {
    if word.contains("://") || word.starts_with("//") || !word.contains('/') {
        return false;
    }
    let last = word.rsplit('/').next().unwrap_or_default();
    match last.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && (1..=5).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && word
                    .chars()
                    .all(|c| c.is_alphanumeric() || "/._-".contains(c))
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(entities: &[Entity]) -> Vec<(EntityKind, &str)> {
        entities.iter().map(|e| (e.kind, e.name.as_str())).collect()
    }

    #[test]
    fn collects_context_lists_and_text_mentions() {
        let context = WorkContext {
            files: vec!["./src\\a.rs".into()],
            tests: vec!["it_works".into()],
            ..WorkContext::default()
        };
        let got = derive(
            &[
                "Edit crates/core/src/lib.rs, run `cargo test -p cairn-core` and call `Memory::save` (see https://x.io/a.html)",
            ],
            &context,
        );
        let got = names(&got);
        assert!(got.contains(&(EntityKind::File, "src/a.rs")));
        assert!(got.contains(&(EntityKind::Test, "it_works")));
        assert!(got.contains(&(EntityKind::File, "crates/core/src/lib.rs")));
        assert!(got.contains(&(EntityKind::Command, "cargo test -p cairn-core")));
        assert!(got.contains(&(EntityKind::Symbol, "Memory::save")));
        assert!(!got.iter().any(|(_, n)| n.contains("x.io")));
    }

    #[test]
    fn deduplicates_and_caps() {
        let context = WorkContext {
            files: (0..60)
                .map(|i| format!("f{i}.rs"))
                .chain(["f0.rs".to_owned()])
                .collect(),
            ..WorkContext::default()
        };
        let got = derive(&[], &context);
        assert_eq!(got.len(), ENTITY_CAP);
        assert_eq!(got.iter().filter(|e| e.name == "f0.rs").count(), 1);
    }
}
