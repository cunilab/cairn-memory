//! Bounds and text evidence for automatic prompt recall.
pub const PROMPT_QUERY_MAX_BYTES: usize = 1024;

#[cfg(test)]
mod tests;

pub fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|word| word.chars().any(char::is_alphanumeric))
}

pub fn should_recall(prompt: &str) -> bool {
    let prompt = prompt.trim_start();
    !prompt.starts_with('/')
        && words(prompt).take(4).count() == 4
        && words(prompt).any(|word| {
            !matches!(
                word.to_ascii_lowercase().as_str(),
                "ok" | "okay"
                    | "yes"
                    | "yep"
                    | "yeah"
                    | "continue"
                    | "go"
                    | "next"
                    | "ahead"
                    | "and"
                    | "on"
                    | "please"
                    | "sure"
                    | "right"
                    | "thanks"
                    | "do"
                    | "it"
                    | "that"
            )
        })
}

/// A short explicit cue can name a preference inside a much longer prompt.
pub fn named_preference<'a>(prompt: &'a str, title: &str, summary: &str) -> Option<&'a str> {
    let mut previous = "";
    for word in words(prompt) {
        if previous.eq_ignore_ascii_case("with")
            && word.len() <= 32
            && word.chars().count() >= 3
            && (words(title)
                .next()
                .is_some_and(|first| first.eq_ignore_ascii_case(word))
                || [title, summary].into_iter().any(|text| {
                    let mut prior: &str = "";
                    words(text).any(|candidate| {
                        let matched = prior.eq_ignore_ascii_case("with")
                            && candidate.eq_ignore_ascii_case(word);
                        prior = candidate;
                        matched
                    })
                }))
        {
            return Some(word);
        }
        previous = word;
    }
    None
}

pub fn matching_word<'a>(prompt: &'a str, title: &str, summary: &str) -> Option<&'a str> {
    [title, summary].into_iter().find_map(|text| {
        words(prompt)
            .filter(|word| word.len() <= 32 && word.chars().count() >= 3)
            .filter(|word| words(text).any(|candidate| candidate.eq_ignore_ascii_case(word)))
            .max_by_key(|word| word.len())
    })
}
