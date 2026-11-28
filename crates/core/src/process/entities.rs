use crate::capture::WorkContext;
use crate::memory::{Entity, EntityKind};

pub const ENTITY_CAP: usize = 32;

/// A command is only recognised in prose when it starts with one of these programs.
const COMMAND_PROGRAMS: [&str; 20] = [
    "cargo", "npm", "pnpm", "yarn", "npx", "node", "git", "docker", "kubectl", "make", "python",
    "python3", "pytest", "go", "psql", "cairn", "bash", "sh", "just", "uv",
];
/// A module that is only one of these says nothing about the code.
const GENERIC_MODULES: [&str; 5] = ["src", "lib", "app", "crates", "packages"];

/// Deterministic entities (apps/docs/content/technical/index.mdx): the request's lists first, then backticked spans, then
/// bare text; the parent directory of every file last. Deduplicated, capped at 32 (first wins).
/// The one function behind save, link jobs and the search entity channel.
pub fn derive(texts: &[&str], context: &WorkContext) -> Vec<Entity> {
    derive_with_evidence(texts, &[], context)
}

/// `derive`, plus the files and commands an evidence list names, after the text's own entities.
pub fn derive_with_evidence(
    texts: &[&str],
    evidence: &[String],
    context: &WorkContext,
) -> Vec<Entity> {
    let mut out: Vec<Entity> = Vec::new();
    let worktree = context
        .worktree
        .as_deref()
        .map(|w| w.trim().replace('\\', "/").trim_end_matches('/').to_owned())
        .filter(|w| !w.is_empty());
    for name in &context.files {
        push_file(&mut out, &normalize_path(name, worktree.as_deref()));
    }
    for name in &context.symbols {
        push_symbol(&mut out, name);
    }
    for name in &context.commands {
        push(&mut out, EntityKind::Command, normalize_command(name));
    }
    for name in &context.tests {
        push(&mut out, EntityKind::Test, clean_symbol(name));
    }
    for name in &context.services {
        push(&mut out, EntityKind::Service, name.trim().to_lowercase());
    }
    for text in texts {
        for (index, part) in text.split('`').enumerate() {
            if index % 2 == 1 {
                push_code(&mut out, part.trim(), worktree.as_deref());
            }
        }
    }
    for text in texts {
        for (index, part) in text.split('`').enumerate() {
            if index % 2 == 0 {
                for word in part.split_whitespace() {
                    push_word(&mut out, word, worktree.as_deref());
                }
            }
        }
    }
    for item in evidence {
        push_evidence(&mut out, item, worktree.as_deref());
    }
    let modules: Vec<String> = out
        .iter()
        .filter(|e| e.kind == EntityKind::File)
        .filter_map(|e| module_of(&e.name))
        .collect();
    for module in modules {
        push(&mut out, EntityKind::Module, module);
    }
    out.truncate(ENTITY_CAP);
    out
}

fn push(out: &mut Vec<Entity>, kind: EntityKind, name: String) {
    if name.is_empty() || name.chars().count() > 200 {
        return;
    }
    if !out.iter().any(|e| e.kind == kind && e.name == name) {
        out.push(Entity { kind, name });
    }
}

/// A file, plus a test entity when the path is a test file.
fn push_file(out: &mut Vec<Entity>, path: &str) {
    push(out, EntityKind::File, path.to_owned());
    if is_test_path(path) {
        push(out, EntityKind::Test, path.to_owned());
    }
}

/// A symbol, or a test when the name says so.
fn push_symbol(out: &mut Vec<Entity>, raw: &str) {
    let name = clean_symbol(raw);
    let kind = if name.starts_with("test_") || name.ends_with("_test") || name.ends_with("Test") {
        EntityKind::Test
    } else {
        EntityKind::Symbol
    };
    push(out, kind, name);
}

/// A backticked span: path, command, symbol or term.
fn push_code(out: &mut Vec<Entity>, code: &str, worktree: Option<&str>) {
    if code.is_empty() {
        return;
    }
    if looks_like_path(code) {
        push_file(out, &normalize_path(code, worktree));
    } else if code.contains(char::is_whitespace) {
        let command = normalize_command(code);
        if command
            .split(' ')
            .next()
            .is_some_and(|program| COMMAND_PROGRAMS.contains(&program))
        {
            push(out, EntityKind::Command, command);
        }
    } else if is_symbol(&clean_symbol(code)) {
        push_symbol(out, code);
    } else if !code.is_empty()
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && code.starts_with(|c: char| c.is_ascii_lowercase())
    {
        push(out, EntityKind::Term, code.to_owned());
    }
}

/// One evidence item, read as a reference: a file (`queue.py`, `src/a.rs:12`) or a command that
/// starts with a known program. Anything else (URLs, commit ids, prose) adds nothing.
fn push_evidence(out: &mut Vec<Entity>, item: &str, worktree: Option<&str>) {
    let item = item.trim();
    if item.contains("://") {
        return;
    }
    // `path:line` and `path:line:column` name the file.
    let mut file = item;
    while let Some((head, tail)) = file.rsplit_once(':') {
        if tail.is_empty() || !tail.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        file = head;
    }
    if looks_like_path(file) || is_bare_file(file) {
        push_file(out, &normalize_path(file, worktree));
        return;
    }
    let command = normalize_command(item);
    if command.contains(' ')
        && command
            .split(' ')
            .next()
            .is_some_and(|program| COMMAND_PROGRAMS.contains(&program))
    {
        push(out, EntityKind::Command, command);
    }
}

/// `queue.py` or `Cargo.toml`: one file name with a short extension and a letter in its stem,
/// so versions such as `0.1.12` are not files.
fn is_bare_file(word: &str) -> bool {
    let Some((stem, ext)) = word.rsplit_once('.') else {
        return false;
    };
    !word.contains('/')
        && stem.chars().any(|c| c.is_ascii_alphabetic())
        && (1..=5).contains(&ext.len())
        && ext.chars().all(|c| c.is_ascii_alphanumeric())
        && ext.chars().any(|c| c.is_ascii_alphabetic())
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || "._-".contains(c))
}

/// Bare prose: paths and symbols only, never inside a URL and never a word of 3 characters or fewer.
fn push_word(out: &mut Vec<Entity>, word: &str, worktree: Option<&str>) {
    // Only the end loses `.`: a leading one is part of a dot directory (`.cairn/project.json`).
    let word = word
        .trim_start_matches(|c: char| ",:()[]\"'!;".contains(c))
        .trim_end_matches(|c: char| ",:()[]\"'!;.".contains(c));
    if word.contains("://") || word.chars().count() <= 3 {
        return;
    }
    if looks_like_path(word) {
        push_file(out, &normalize_path(word, worktree));
    } else if is_symbol(word) {
        push_symbol(out, word);
    }
}

fn normalize_path(path: &str, worktree: Option<&str>) -> String {
    let path = path.trim().replace('\\', "/");
    let path = match worktree.and_then(|w| path.strip_prefix(&format!("{w}/"))) {
        Some(rest) => rest,
        None => path.as_str(),
    };
    let mut path = path;
    while let Some(rest) = path.strip_prefix("./") {
        path = rest;
    }
    path.trim_end_matches('/').to_owned()
}

/// Collapses spaces, cuts at the first pipe, `&&`, `;` or `>`, keeps the first three words.
pub fn normalize_command(command: &str) -> String {
    let cut = ["|", "&&", ";", ">"]
        .iter()
        .filter_map(|stop| command.find(stop))
        .min()
        .unwrap_or(command.len());
    command[..cut]
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The files a command runs or reads, named in its first three words and normalized like file
/// entities (`python3 ./src/a.py` -> `src/a.py`).
pub fn command_files(command: &str) -> Vec<String> {
    normalize_command(command)
        .split(' ')
        .skip(1)
        .filter(|word| looks_like_path(word) || is_bare_file(word))
        .map(|word| normalize_path(word, None))
        .collect()
}

/// Strips the trailing `()`, `!` or `;` of a symbol.
fn clean_symbol(name: &str) -> String {
    let mut name = name.trim();
    loop {
        let next = name
            .strip_suffix("()")
            .or_else(|| name.strip_suffix('!'))
            .or_else(|| name.strip_suffix(';'));
        match next {
            Some(shorter) => name = shorter.trim_end(),
            None => return name.to_owned(),
        }
    }
}

fn ident(part: &str) -> bool {
    part.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `a::b`, camelCase, PascalCase with two humps, or snake_case with two parts.
pub fn is_symbol(name: &str) -> bool {
    let alnum = name.chars().all(|c| c.is_ascii_alphanumeric());
    let mut chars = name.chars();
    let (first, second) = (chars.next(), chars.next());
    let path_qualified =
        name.contains("::") && name.split("::").count() >= 2 && name.split("::").all(ident);
    let camel = alnum
        && first.is_some_and(|c| c.is_ascii_lowercase())
        && name.chars().any(|c| c.is_ascii_uppercase());
    let pascal = alnum
        && first.is_some_and(|c| c.is_ascii_uppercase())
        && second.is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.chars().skip(2).any(|c| c.is_ascii_uppercase());
    let snake = {
        let parts: Vec<&str> = name.split('_').collect();
        parts.len() >= 2
            && parts[0].starts_with(|c: char| c.is_ascii_lowercase())
            && parts[0]
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && parts[1..].iter().all(|p| {
                !p.is_empty()
                    && p.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            })
            && !parts[1..]
                .iter()
                .all(|p| p.chars().all(|c| c.is_ascii_digit()))
    };
    path_qualified || camel || pascal || snake
}

fn is_test_path(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = file.split('.').next().unwrap_or(file);
    path.starts_with("tests/")
        || path.contains("/tests/")
        || stem.ends_with("_test")
        || file.contains(".test.")
        || (stem.starts_with("test_") && file.ends_with(".py"))
}

/// The parent directory, unless it is empty or only a generic top-level name.
pub fn module_of(path: &str) -> Option<String> {
    let (parent, _) = path.rsplit_once('/')?;
    (!parent.is_empty() && !GENERIC_MODULES.contains(&parent)).then(|| parent.to_owned())
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
    fn evidence_names_files_and_commands_after_the_text() {
        let context = WorkContext {
            worktree: Some("/w/repo".into()),
            ..WorkContext::default()
        };
        let evidence: Vec<String> = [
            "python3 violet_latch.py",
            "queue.py",
            "/w/repo/src/a.rs:12:4",
            "https://x.io/a.py",
            "commit 1a2b3c4",
            "0.1.12",
            "see the log",
            "deadbeefcafe",
        ]
        .map(String::from)
        .to_vec();
        let got = derive_with_evidence(&["Use `Store::save`"], &evidence, &context);
        assert_eq!(
            names(&got),
            vec![
                (EntityKind::Symbol, "Store::save"),
                (EntityKind::Command, "python3 violet_latch.py"),
                (EntityKind::File, "queue.py"),
                (EntityKind::File, "src/a.rs"),
            ]
        );
        assert_eq!(derive(&["Use `Store::save`"], &context).len(), 1);
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
        assert!(got.contains(&(EntityKind::Command, "cargo test -p")));
        assert!(got.contains(&(EntityKind::Symbol, "Memory::save")));
        assert!(!got.iter().any(|(_, n)| n.contains("x.io")));
    }

    fn has(entities: &[Entity], kind: EntityKind, name: &str) -> bool {
        entities.iter().any(|e| e.kind == kind && e.name == name)
    }

    fn from_text(text: &str) -> Vec<Entity> {
        derive(&[text], &WorkContext::default())
    }

    #[test]
    fn entities_rows() {
        // file: context + worktree strip + backticked + bare; a URL is not a file.
        let context = WorkContext {
            files: vec!["/w/repo/./src/a.rs".into(), "/w/repo/docs/".into()],
            worktree: Some("/w/repo/".into()),
            ..WorkContext::default()
        };
        let got = derive(
            &["edit `crates/x/lib.rs` and src/b.rs, not https://x.io/c.html"],
            &context,
        );
        assert!(has(&got, EntityKind::File, "src/a.rs"));
        assert!(has(&got, EntityKind::File, "docs"));
        assert!(has(&got, EntityKind::File, "crates/x/lib.rs"));
        assert!(has(&got, EntityKind::File, "src/b.rs"));
        assert!(!got.iter().any(|e| e.name.contains("x.io")));
        // module: parent directory of a file.
        assert!(has(&got, EntityKind::Module, "crates/x"));
        // symbol: the four shapes, with trailing punctuation stripped; negatives stay out.
        let got = from_text(
            "`Memory::save()` `parseThing` `OrderItem` `retry_budget` `plain` `ALLCAPS` `a_1` `Order` `abc`",
        );
        for name in ["Memory::save", "parseThing", "OrderItem", "retry_budget"] {
            assert!(has(&got, EntityKind::Symbol, name), "{name}");
        }
        for name in ["ALLCAPS", "a_1", "Order"] {
            assert!(!got.iter().any(|e| e.name == name), "{name}");
        }
        assert!(has(&got, EntityKind::Term, "plain"));
        assert!(has(&got, EntityKind::Term, "abc"));
        // bare symbols need more than 3 characters and are skipped inside URLs.
        let got = from_text("call load_user then fooBar; see https://x.io/some_thing_else and a_b");
        assert!(has(&got, EntityKind::Symbol, "load_user"));
        assert!(has(&got, EntityKind::Symbol, "fooBar"));
        assert!(
            !got.iter()
                .any(|e| e.name.contains("some_thing") || e.name == "a_b")
        );
        // test: context, name rules, test file paths (file kept too).
        let context = WorkContext {
            tests: vec!["it_works".into()],
            symbols: vec!["test_parse".into(), "ParserTest".into(), "helper_fn".into()],
            files: vec![
                "tests/common/db.rs".into(),
                "src/a_test.rs".into(),
                "web/b.test.ts".into(),
                "src/lib.rs".into(),
            ],
            ..WorkContext::default()
        };
        let got = derive(&[], &context);
        for name in [
            "it_works",
            "test_parse",
            "ParserTest",
            "tests/common/db.rs",
            "src/a_test.rs",
            "web/b.test.ts",
        ] {
            assert!(has(&got, EntityKind::Test, name), "{name}");
        }
        assert!(has(&got, EntityKind::File, "tests/common/db.rs"));
        assert!(has(&got, EntityKind::Symbol, "helper_fn"));
        assert!(!has(&got, EntityKind::Test, "src/lib.rs"));
        // command: allowed program only, cut at pipes/&&/;/>, first three words.
        let got = from_text(
            "run `cargo   test -p cairn-core --lib | tail` then `rm -rf build` and `git status && git diff`",
        );
        assert!(has(&got, EntityKind::Command, "cargo test -p"));
        assert!(has(&got, EntityKind::Command, "git status"));
        assert!(
            !got.iter()
                .any(|e| e.kind == EntityKind::Command && e.name.starts_with("rm"))
        );
        let context = WorkContext {
            commands: vec!["make  build > out.txt".into()],
            ..WorkContext::default()
        };
        assert!(has(
            &derive(&[], &context),
            EntityKind::Command,
            "make build"
        ));
        // service: lower-cased from the context only.
        let context = WorkContext {
            services: vec![" Postgres ".into()],
            ..WorkContext::default()
        };
        let got = derive(&["Postgres"], &context);
        assert!(has(&got, EntityKind::Service, "postgres"));
        assert!(!has(&got, EntityKind::Service, "Postgres"));
        // term: a backticked single lower-case word.
        assert!(has(
            &from_text("the `tokio` runtime"),
            EntityKind::Term,
            "tokio"
        ));
        assert!(
            !from_text("the tokio runtime")
                .iter()
                .any(|e| e.kind == EntityKind::Term)
        );
    }

    #[test]
    fn bare_dot_directory_paths_keep_their_leading_dot() {
        // The live false-stale gotcha: `.cairn/project.json` was cut to `cairn/project.json`.
        let got = from_text(
            "Committed .cairn/project.json points at a project id; also (./.github/ci.yml), see src/a.rs.",
        );
        assert!(has(&got, EntityKind::File, ".cairn/project.json"));
        assert!(has(&got, EntityKind::Module, ".cairn"));
        assert!(has(&got, EntityKind::File, ".github/ci.yml"));
        assert!(has(&got, EntityKind::File, "src/a.rs"));
        assert!(!has(&got, EntityKind::File, "cairn/project.json"));
        // Relative parents stay outside the repo (judged by IN_REPO), not mangled into `/x.rs`.
        assert!(has(
            &from_text("see ../x/y.rs now"),
            EntityKind::File,
            "../x/y.rs"
        ));
    }

    #[test]
    fn module_skips_top_level_src() {
        let context = WorkContext {
            files: vec![
                "src/lib.rs".into(),
                "Cargo.toml".into(),
                "app/page.tsx".into(),
                "src/auth/session.rs".into(),
            ],
            ..WorkContext::default()
        };
        let modules: Vec<_> = derive(&[], &context)
            .into_iter()
            .filter(|e| e.kind == EntityKind::Module)
            .map(|e| e.name)
            .collect();
        assert_eq!(modules, ["src/auth"]);
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
