//! Explicit-search query text. The original query is unchanged for the gate and for hooks.
//! The lexical any-term leg may also search parts of an unprotected identifier.

const MAX_PARTS: usize = 8;
const MIN_PART: usize = 4;

/// `OR` terms for identifier parts. Empty when there is nothing safe to add.
/// Quoted text, version numbers (`1.4.2`) and a line that contains `error:` or `error[`
/// are not split. A leading `-` (a negated websearch term) adds nothing.
pub fn lexical_extras(query: &str) -> String {
    if query
        .split_whitespace()
        .any(|word| word.starts_with('-') && word.len() > 1)
    {
        return String::new();
    }
    let mut parts = Vec::new();
    for token in bare_tokens(&mask_protected(query)) {
        for part in split_token(&token) {
            if part.len() >= MIN_PART && !parts.iter().any(|seen: &String| seen == &part) {
                parts.push(part);
            }
            if parts.len() == MAX_PARTS {
                return parts.join(" OR ");
            }
        }
    }
    parts.join(" OR ")
}

/// Whole paths, symbols, versions, quoted phrases and error lines that can win a tie.
pub fn exact_anchors(query: &str) -> Vec<String> {
    let mut anchors = Vec::new();
    let mut push = |text: String| {
        let text = text.trim().to_owned();
        if text.len() >= 3 && !anchors.iter().any(|seen: &String| seen == &text) {
            anchors.push(text);
        }
    };
    for (inner, _) in quoted_spans(query) {
        push(inner);
    }
    for line in query.lines() {
        if is_error_line(line) {
            push(line.to_owned());
        }
    }
    for token in bare_tokens(query) {
        if is_version(&token) || is_snake(&token) || is_camel(&token) || is_path(&token) {
            push(token);
        }
    }
    anchors
}

/// An anchor names one of `names`, or an error line occurs in `text`.
pub fn anchor_matches(anchors: &[String], names: &[&str], text: &str) -> bool {
    anchors.iter().any(|anchor| {
        names.iter().any(|name| name.eq_ignore_ascii_case(anchor))
            || (is_error_line(anchor) && text.contains(anchor.as_str()))
    })
}

fn mask_protected(query: &str) -> String {
    let mut chars: Vec<char> = query.chars().collect();
    let mut quoted = false;
    for ch in &mut chars {
        if *ch == '"' {
            quoted = !quoted;
            *ch = ' ';
            continue;
        }
        if quoted {
            *ch = ' ';
        }
    }
    let masked: String = chars.into_iter().collect();
    let mut out = String::new();
    for line in masked.split_inclusive('\n') {
        let raw = line.trim_end_matches('\n');
        if is_error_line(raw) {
            out.extend(std::iter::repeat_n(' ', raw.chars().count()));
        } else {
            out.push_str(&blank_versions(raw));
        }
        if line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn blank_versions(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut skip = vec![false; chars.len()];
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit()
            && let Some(end) = version_end(&chars, i)
        {
            for mark in skip.iter_mut().take(end).skip(i) {
                *mark = true;
            }
            i = end;
            continue;
        }
        i += 1;
    }
    chars
        .into_iter()
        .zip(skip)
        .map(|(ch, hide)| if hide { ' ' } else { ch })
        .collect()
}

/// `1.4` or `1.4.2` starting at `i`. Returns the index after the token.
fn version_end(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    let mut dots = 0;
    while j < chars.len() && chars[j].is_ascii_digit() {
        j += 1;
    }
    if j == i {
        return None;
    }
    while j < chars.len() && chars[j] == '.' {
        let dot = j;
        j += 1;
        let start = j;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        if j == start {
            return None;
        }
        dots += 1;
        if dot > 0 && !chars.get(j).is_none_or(|c| !c.is_ascii_alphanumeric()) {
            return None;
        }
    }
    (dots >= 1).then_some(j)
}

fn quoted_spans(query: &str) -> Vec<(String, ())> {
    let mut out = Vec::new();
    let mut rest = query;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('"') {
            out.push((rest[..end].to_owned(), ()));
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    out
}

fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error:") || lower.contains("error[")
}

fn bare_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '/' || c == '.'))
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

fn split_token(token: &str) -> Vec<String> {
    let stem = if is_path(token) {
        path_stem(token)
    } else if is_snake(token) || is_camel(token) {
        token
    } else {
        return Vec::new();
    };
    let mut parts = Vec::new();
    if is_snake(stem) {
        parts.extend(stem.split('_').map(|part| part.to_ascii_lowercase()));
    } else if is_camel(stem) {
        parts.extend(camel_parts(stem));
    }
    parts.retain(|part| part != stem);
    parts
}

fn path_stem(token: &str) -> &str {
    let file = token.rsplit('/').next().unwrap_or(token);
    file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(file)
}

fn is_path(token: &str) -> bool {
    token.contains('/')
        && !token.contains("://")
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
        && token
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains('.'))
}

fn is_version(token: &str) -> bool {
    let chars: Vec<char> = token.chars().collect();
    version_end(&chars, 0) == Some(chars.len())
}

fn is_snake(token: &str) -> bool {
    let parts: Vec<&str> = token.split('_').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric()))
        && token.starts_with(|c: char| c.is_ascii_alphabetic())
}

fn is_camel(token: &str) -> bool {
    token.chars().all(|c| c.is_ascii_alphanumeric())
        && token.chars().any(|c| c.is_ascii_lowercase())
        && token.chars().any(|c| c.is_ascii_uppercase())
        && !token.contains('_')
}

fn camel_parts(token: &str) -> Vec<String> {
    let chars: Vec<char> = token.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    for i in 1..chars.len() {
        let boundary = chars[i].is_ascii_uppercase()
            && (!chars[i - 1].is_ascii_uppercase()
                || chars.get(i + 1).is_some_and(|c| c.is_ascii_lowercase()));
        if boundary {
            parts.push(
                chars[start..i]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase(),
            );
            start = i;
        }
    }
    parts.push(
        chars[start..]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase(),
    );
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_snake_camel_and_a_path_stem() {
        assert_eq!(lexical_extras("quartz_claim"), "quartz OR claim");
        assert_eq!(lexical_extras("quartzClaim"), "quartz OR claim");
        assert_eq!(
            lexical_extras("see src/jobs/quartz_queue.rs"),
            "quartz OR queue"
        );
    }

    #[test]
    fn protects_quotes_versions_errors_and_negation() {
        assert_eq!(lexical_extras("\"quartz_claim\" stays whole"), "");
        assert_eq!(lexical_extras("harbor-cli 1.4.2"), "");
        assert_eq!(
            lexical_extras("error: moved quartz_claim before the update"),
            ""
        );
        assert_eq!(lexical_extras("drop table -guitar"), "");
    }

    #[test]
    fn anchors_are_the_whole_token_and_match_a_name_or_an_error() {
        let anchors = exact_anchors("src/jobs/quartz_queue.rs and 1.4.2");
        assert!(anchors.iter().any(|a| a == "src/jobs/quartz_queue.rs"));
        assert!(anchors.iter().any(|a| a == "1.4.2"));
        assert!(anchor_matches(&anchors, &["src/jobs/quartz_queue.rs"], ""));
        let error = exact_anchors("error: the worker panicked after moving quartz claim");
        assert!(anchor_matches(
            &error,
            &[],
            "error: the worker panicked after moving quartz claim"
        ));
        assert!(!anchor_matches(&error, &[], "unrelated note"));
    }
}
