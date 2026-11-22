use super::model::Kind;
use crate::id::sha256_hex;
use serde_json::Value;

/// Trimmed, non-empty, first-seen order, no repeats.
pub fn normalize_evidence(evidence: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in evidence {
        let item = item.trim();
        if !item.is_empty() && !out.iter().any(|seen| seen == item) {
            out.push(item.to_owned());
        }
    }
    out
}

/// SHA-256 of the sorted, deduplicated evidence.
pub fn evidence_hash(evidence: &[String]) -> String {
    let mut sorted = normalize_evidence(evidence);
    sorted.sort();
    let parts: Vec<&str> = sorted.iter().map(String::as_str).collect();
    sha256_hex(&parts)
}

/// Lowercase with runs of whitespace collapsed, so a re-save that only differs in case or
/// spacing is the same memory.
fn squash(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// SHA-256 of `kind, title, summary, fields` (title and summary squashed). The reason is not
/// part of the identity: the same note saved again with another reason is a duplicate, not a
/// new version. Inputs must already be redacted.
pub fn identity_hash(kind: Kind, title: &str, summary: &str, fields: &Value) -> String {
    sha256_hex(&[
        kind.as_str(),
        &squash(title),
        &squash(summary),
        &fields.to_string(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn evidence_hash_ignores_order_and_repeats() {
        let a = evidence_hash(&["b".into(), "a".into(), "a".into()]);
        assert_eq!(a, evidence_hash(&["a".into(), "b".into()]));
        assert_ne!(a, evidence_hash(&["a".into()]));
    }

    #[test]
    fn identity_changes_with_any_part() {
        let f = json!({"source": "x"});
        let base = identity_hash(Kind::Fact, "t", "s", &f);
        assert_eq!(base, identity_hash(Kind::Fact, "t", "s", &f));
        assert_ne!(base, identity_hash(Kind::Rule, "t", "s", &f));
        assert_ne!(base, identity_hash(Kind::Fact, "t", "s2", &f));
        assert_ne!(base, identity_hash(Kind::Fact, "t", "s", &json!({})));
        // Case and spacing do not make a new identity.
        assert_eq!(base, identity_hash(Kind::Fact, " T ", "S", &f));
        assert_eq!(
            identity_hash(Kind::Fact, "A  b", "c\nd", &f),
            identity_hash(Kind::Fact, "a b", "c d", &f)
        );
    }
}
