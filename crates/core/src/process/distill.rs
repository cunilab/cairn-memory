use super::entities::normalize_command;
use crate::capture::{CaptureKind, Observation, WorkContext};
use crate::id::sha256_hex;
use crate::memory::validate::validate_quality;
use crate::memory::validate::{
    FIELD_TEXT_MAX, LIST_ITEM_MAX, LIST_MAX_ITEMS, SUMMARY_MAX, TITLE_MAX,
};
use crate::memory::{Fields, Kind, SaveInput, Scope};
use serde_json::{Map, Value, json};

const MAX_TYPED_NOTES: usize = 64;
const MAX_GOTCHAS: usize = 3;
pub const HANDOFF_FILES_MAX: usize = 20;
/// Normalised commands starting with one of these are "the test command" (D3).
const TEST_COMMANDS: [&str; 13] = [
    "cargo test",
    "cargo nextest",
    "npm test",
    "npm run test",
    "pnpm test",
    "yarn test",
    "bun test",
    "bunx vitest",
    "pytest",
    "python -m pytest",
    "go test",
    "make test",
    "just test",
];

/// One distilled note and the files it is about (its entity context).
#[derive(Clone, Debug)]
pub struct Note {
    pub input: SaveInput,
    pub files: Vec<String>,
}

/// Rule-based output of one session: typed notes from `Decision:`-style lines, a fail-then-fix
/// gotcha, a test-command fact, a handoff, and one session digest when files changed. All are
/// plain `SaveInput`s built to pass the same validation and quality rules as an agent's save.
#[derive(Clone, Debug, Default)]
pub struct Distilled {
    pub notes: Vec<Note>,
    pub handoff: Option<SaveInput>,
    /// One note for the session when it changed files. `None` when it did not.
    pub digest: Option<SaveInput>,
}

/// One handoff per (owner, project-or-repository, branch, worktree): the id is derived, so a
/// later distill in the same checkout replaces it.
pub fn handoff_id(person_id: &str, ctx: &WorkContext) -> String {
    let place = ctx
        .project_id
        .as_deref()
        .or(ctx.repository.as_deref())
        .unwrap_or("");
    let parts = [
        person_id,
        place,
        ctx.branch.as_deref().unwrap_or(""),
        ctx.worktree.as_deref().unwrap_or(""),
    ];
    format!("hnd_{}", &sha256_hex(&parts)[..32])
}

pub fn distill(observations: &[Observation]) -> Distilled {
    let text: Vec<&Observation> = observations
        .iter()
        .filter(|o| matches!(o.kind, CaptureKind::Prompt | CaptureKind::Assistant))
        .collect();
    let mut notes: Vec<Note> = typed_notes(&text)
        .into_iter()
        .map(|(input, files)| Note { input, files })
        .collect();
    let failures = failures(observations);
    notes.extend(gotchas(observations, &failures));
    notes.extend(test_fact(observations));
    // System saves skip the agent quality check, so what would fail it is not worth keeping.
    notes.retain(|n| passes_quality(&n.input));
    Distilled {
        notes,
        handoff: handoff(observations, &failures),
        digest: digest(observations),
    }
}

fn passes_quality(input: &SaveInput) -> bool {
    Fields::parse(input.kind, input.fields.clone())
        .is_ok_and(|fields| validate_quality(input, &fields).is_ok())
}

/// One failed command and what followed it.
struct Failure<'a> {
    key: String,
    observation: &'a Observation,
    index: usize,
}

fn command_text(o: &Observation) -> &str {
    o.context
        .commands
        .first()
        .map(String::as_str)
        .or_else(|| o.content.lines().next())
        .unwrap_or("")
}

/// The first two words of the normalised command: `cargo test` for `cargo test --workspace`.
fn key_of(o: &Observation) -> String {
    normalize_command(command_text(o))
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text after `exit <n>: ` (or `failed: `) on the second line of a failed command capture.
fn error_line(o: &Observation) -> Option<String> {
    let line = o.content.lines().nth(1)?;
    let (head, rest) = line.split_once(": ")?;
    (head.starts_with("exit ") || head == "failed").then(|| rest.trim().to_owned())
}

fn failures(observations: &[Observation]) -> Vec<Failure<'_>> {
    observations
        .iter()
        .enumerate()
        .filter(|(_, o)| o.kind == CaptureKind::Command && o.ok == Some(false))
        .map(|(index, o)| Failure {
            key: key_of(o),
            observation: o,
            index,
        })
        .filter(|f| !f.key.is_empty())
        .collect()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// D2: for each failed key, the first later passing command with an edit between them.
fn gotchas(observations: &[Observation], failures: &[Failure<'_>]) -> Vec<Note> {
    let mut seen: Vec<&str> = Vec::new();
    let mut notes = Vec::new();
    for failure in failures {
        if notes.len() >= MAX_GOTCHAS || seen.contains(&failure.key.as_str()) {
            continue;
        }
        let Some(pass) = observations[failure.index + 1..].iter().position(|o| {
            o.kind == CaptureKind::Command && o.ok == Some(true) && key_of(o) == failure.key
        }) else {
            continue;
        };
        let pass = failure.index + 1 + pass;
        let edits: Vec<&Observation> = observations[failure.index + 1..pass]
            .iter()
            .filter(|o| o.kind == CaptureKind::Edit)
            .collect();
        if edits.is_empty() {
            continue;
        }
        seen.push(&failure.key);
        let mut files = Vec::new();
        let mut evidence = vec![failure.observation.event_id.clone()];
        for edit in &edits {
            for file in &edit.context.files {
                push_unique(&mut files, file);
            }
            push_unique(&mut evidence, &edit.event_id);
        }
        push_unique(&mut evidence, &observations[pass].event_id);
        let key = &failure.key;
        let names: Vec<&str> = files.iter().take(2).map(|f| basename(f)).collect();
        let error = error_line(failure.observation).filter(|e| e.chars().count() >= 20);
        let edited: Vec<&str> = files.iter().take(3).map(String::as_str).collect();
        let symptom = error_line(failure.observation)
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| format!("`{key}` exited non-zero"));
        let fix = format!(
            "Edited {}",
            files
                .iter()
                .take(5)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        );
        notes.push(Note {
            input: SaveInput {
                kind: Kind::Gotcha,
                title: clip(
                    &format!("`{key}` failed until {} changed", names.join(", ")),
                    TITLE_MAX,
                ),
                summary: clip(
                    &error.unwrap_or_else(|| {
                        format!(
                            "`{key}` failed, then passed after editing {}.",
                            edited.join(", ")
                        )
                    }),
                    SUMMARY_MAX,
                ),
                reason:
                    "Seen in one session: the command failed, files were edited, then it passed."
                        .to_owned(),
                fields: Some(json!({
                    "symptom": clip(&symptom, FIELD_TEXT_MAX),
                    "fix": clip(&fix, FIELD_TEXT_MAX),
                })),
                evidence,
                supersedes: None,
                contradicts: None,
                scope: None,
            },
            files,
        });
    }
    notes
}

/// Whether a shell command is a recognised test command (D3).
pub fn is_test_command(command: &str) -> bool {
    let command = normalize_command(command);
    TEST_COMMANDS
        .iter()
        .any(|t| command == *t || command.strip_prefix(t).is_some_and(|r| r.starts_with(' ')))
}

/// Whether a text has a `Decision:`-style line (any typed label with a value), the lines the
/// distiller turns into notes.
pub fn has_typed_line(text: &str) -> bool {
    text.lines().any(|line| {
        line.trim().split_once(':').is_some_and(|(label, value)| {
            typed_kind(&label.trim().to_ascii_lowercase()).is_some() && !value.trim().is_empty()
        })
    })
}

const CORRECTION_PREFIXES: &[&str] = &[
    "no,",
    "no ",
    "don't ",
    "do not ",
    "never ",
    "always ",
    "stop ",
    "please don't ",
    "please never ",
    "please always ",
    "wrong",
    "that's wrong",
    "i told you",
];

/// A correction starts with one of [`CORRECTION_PREFIXES`] (lower-case, start of the line).
pub fn is_correction(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    CORRECTION_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// The first correction line in a prompt. Fenced code and boilerplate lines are skipped.
pub fn first_correction(text: &str) -> Option<String> {
    let mut fenced = false;
    for raw in text.lines() {
        let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced || line.is_empty() || is_boilerplate(&line) {
            continue;
        }
        if is_correction(&line) {
            return Some(line);
        }
    }
    None
}

/// D3: the first passing test command of the session.
fn test_fact(observations: &[Observation]) -> Option<Note> {
    let passed = observations.iter().find(|o| {
        o.kind == CaptureKind::Command && o.ok == Some(true) && is_test_command(command_text(o))
    })?;
    let command = normalize_command(command_text(passed));
    Some(Note {
        input: SaveInput {
            kind: Kind::Fact,
            title: clip(
                &format!("Tests run with `{}`", clip(&command, 50)),
                TITLE_MAX,
            ),
            summary: clip(
                &format!("`{command}` is the test command that passed in this repository."),
                SUMMARY_MAX,
            ),
            reason: "Observed a passing run in a session.".to_owned(),
            fields: Some(json!({ "source": "session capture" })),
            evidence: vec![passed.event_id.clone()],
            supersedes: None,
            contradicts: None,
            scope: None,
        },
        files: Vec::new(),
    })
}

/// One searchable note for a session that changed files: the paths, and the last result of each
/// recognised test command. Prompt text stays out.
fn digest(observations: &[Observation]) -> Option<SaveInput> {
    let mut files = Vec::new();
    for observation in observations {
        if observation.kind != CaptureKind::Edit {
            continue;
        }
        for file in &observation.context.files {
            let file = file.trim();
            if !file.is_empty() && !files.iter().any(|seen| seen == file) {
                files.push(file.to_owned());
            }
        }
    }
    if files.is_empty() {
        return None;
    }
    let mut results: Vec<(String, bool)> = Vec::new();
    for observation in observations {
        if observation.kind != CaptureKind::Command {
            continue;
        }
        let Some(ok) = observation.ok else {
            continue;
        };
        if !is_test_command(command_text(observation)) {
            continue;
        }
        let key = key_of(observation);
        if key.is_empty() {
            continue;
        }
        if let Some(slot) = results.iter_mut().find(|(name, _)| name == &key) {
            slot.1 = ok;
        } else {
            results.push((key, ok));
        }
    }
    let listed = if files.len() > 6 {
        format!(
            "{}, +{} more",
            files
                .iter()
                .take(6)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            files.len() - 6
        )
    } else {
        files.join(", ")
    };
    let tests = results
        .iter()
        .take(3)
        .map(|(command, ok)| format!("{command} {}", if *ok { "pass" } else { "fail" }))
        .collect::<Vec<_>>()
        .join(", ");
    let last = observations.last()?;
    let place = last
        .context
        .branch
        .clone()
        .or_else(|| {
            last.context.repository.as_deref().map(|repository| {
                basename(repository.strip_prefix("local:").unwrap_or(repository)).to_owned()
            })
        })
        .unwrap_or_else(|| "workspace".to_owned());
    let mut evidence = Vec::new();
    for file in &files {
        push_unique(&mut evidence, file);
    }
    for (command, _) in results.iter().take(3) {
        push_unique(&mut evidence, command);
    }
    Some(SaveInput {
        kind: Kind::Note,
        title: clip(
            &format!("Session {} on {place}", date(last.captured_at)),
            TITLE_MAX,
        ),
        summary: clip(
            &if tests.is_empty() {
                format!("Changed {listed}.")
            } else {
                format!("Changed {listed}. Tests: {tests}.")
            },
            SUMMARY_MAX,
        ),
        reason: "Derived from the captures of one session.".to_owned(),
        fields: None,
        evidence,
        supersedes: None,
        contradicts: None,
        scope: None,
    })
}

/// D4: what the next session needs: edited files, marker lines from prompts, failures still open.
fn handoff(observations: &[Observation], failures: &[Failure<'_>]) -> Option<SaveInput> {
    let mut next_steps = Vec::new();
    let mut open = Vec::new();
    let mut files = Vec::new();
    let mut evidence = Vec::new();
    for observation in observations {
        if observation.kind == CaptureKind::Edit {
            for file in &observation.context.files {
                if files.len() < HANDOFF_FILES_MAX {
                    push_unique(&mut files, file);
                }
            }
        }
        if observation.kind != CaptureKind::Prompt {
            continue;
        }
        let mut fenced = false;
        for line in observation.content.lines() {
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            if line.starts_with("```") {
                fenced = !fenced;
                continue;
            }
            if fenced || is_boilerplate(&line) {
                continue;
            }
            let lower = line.to_ascii_lowercase();
            let mut used = false;
            if is_next_step(&line) {
                used |= push_unique(&mut next_steps, &line);
            }
            if has_marker(
                &lower,
                &[
                    "todo",
                    "unresolved",
                    "blocked",
                    "not yet",
                    "not covered",
                    "untested",
                    "follow up",
                    "follow-up",
                    "remaining",
                ],
            ) {
                used |= push_unique(&mut open, &line);
            }
            if used {
                push_unique(&mut evidence, &observation.event_id);
            }
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for failure in failures {
        let later_pass = observations[failure.index + 1..].iter().any(|o| {
            o.kind == CaptureKind::Command && o.ok == Some(true) && key_of(o) == failure.key
        });
        if later_pass || seen.contains(&failure.key.as_str()) {
            continue;
        }
        seen.push(&failure.key);
        let error = error_line(failure.observation).unwrap_or_default();
        push_unique(
            &mut open,
            &format!("`{}` still failing: {error}", failure.key),
        );
        push_unique(&mut evidence, &failure.observation.event_id);
    }
    let last = observations.last()?;
    let edited = observations.iter().any(|o| o.kind == CaptureKind::Edit);
    if !edited && next_steps.is_empty() && open.is_empty() {
        return None;
    }
    let outside = observations
        .iter()
        .filter(|observation| observation.kind == CaptureKind::Edit)
        .map(|observation| observation.context.outside_files)
        .max()
        .unwrap_or(0);
    let place = last
        .context
        .branch
        .clone()
        .or_else(|| {
            last.context
                .repository
                .as_deref()
                .map(|r| basename(r.strip_prefix("local:").unwrap_or(r)).to_owned())
        })
        .unwrap_or_else(|| "workspace".to_owned());
    Some(SaveInput {
        kind: Kind::Handoff,
        title: clip(&format!("Handoff: {place}"), TITLE_MAX),
        summary: clip(
            &handoff_summary(files.len(), outside, open.len(), &date(last.captured_at)),
            SUMMARY_MAX,
        ),
        reason: "Derived from the captures of one session.".to_owned(),
        fields: Some(json!({ "next_steps": next_steps, "open_questions": open, "files": files })),
        evidence,
        supersedes: None,
        contradicts: None,
        scope: Some(Scope::Personal),
    })
}

/// `YYYY-MM-DD` (UTC) of an epoch second, by the civil-from-days algorithm.
fn date(seconds: i64) -> String {
    let z = seconds.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn typed_notes(observations: &[&Observation]) -> Vec<(SaveInput, Vec<String>)> {
    let mut notes: Vec<Draft> = Vec::new();
    'sources: for observation in observations {
        let mut current: Option<Draft> = None;
        for line in observation.content.lines() {
            let Some((label, value)) = line.trim().split_once(':') else {
                notes.extend(current.take());
                continue;
            };
            let label = label.trim().to_ascii_lowercase();
            let value = value.trim();
            if let Some(kind) = typed_kind(&label) {
                notes.extend(current.take());
                if notes.len() >= MAX_TYPED_NOTES {
                    break 'sources;
                }
                if !value.is_empty() {
                    current = Some(Draft::new(kind, value, observation));
                }
            } else if let Some(draft) = current.as_mut()
                && draft.take_field(&label, value)
            {
                // Metadata must immediately follow its typed note; prose ends it.
            } else {
                notes.extend(current.take());
            }
        }
        notes.extend(current);
        if notes.len() >= MAX_TYPED_NOTES {
            break;
        }
    }
    notes.into_iter().map(Draft::into_note).collect()
}

fn typed_kind(label: &str) -> Option<Kind> {
    match label {
        "decision" => Some(Kind::Decision),
        "gotcha" => Some(Kind::Gotcha),
        "procedure" => Some(Kind::Procedure),
        "rule" => Some(Kind::Rule),
        "fact" => Some(Kind::Fact),
        _ => None,
    }
}

struct Draft {
    kind: Kind,
    value: String,
    reason: String,
    fields: Map<String, Value>,
    event_id: String,
    files: Vec<String>,
}

impl Draft {
    fn new(kind: Kind, value: &str, observation: &Observation) -> Self {
        Self {
            kind,
            value: value.to_owned(),
            reason: String::new(),
            fields: Map::new(),
            event_id: observation.event_id.clone(),
            files: observation.context.files.clone(),
        }
    }

    /// Maps a metadata label to this kind's field. False when the label does not belong.
    fn take_field(&mut self, label: &str, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        let key = match (self.kind, label) {
            (_, "reason") => {
                self.reason = value.to_owned();
                return true;
            }
            (Kind::Decision, "decision context") => "context",
            (Kind::Decision, "consequences") => "consequences",
            (Kind::Decision, "alternative" | "alternatives") => {
                return self.push("alternatives", value);
            }
            (Kind::Rule, "exceptions") => "exceptions",
            (Kind::Gotcha, "symptom") => "symptom",
            (Kind::Gotcha, "cause") => "cause",
            (Kind::Gotcha, "fix") => "fix",
            (Kind::Procedure, "step") => return self.push("steps", value),
            (Kind::Procedure, "when") => "when",
            (Kind::Fact, "source") => "source",
            _ => return false,
        };
        self.fields
            .insert(key.to_owned(), Value::String(value.to_owned()));
        true
    }

    fn push(&mut self, key: &str, value: &str) -> bool {
        let list = self.fields.entry(key).or_insert_with(|| json!([]));
        if let Value::Array(items) = list
            && items.len() < LIST_MAX_ITEMS
        {
            items.push(Value::String(clip(value, LIST_ITEM_MAX)));
        }
        true
    }

    fn into_note(self) -> (SaveInput, Vec<String>) {
        let input = SaveInput {
            kind: self.kind,
            title: clip(&self.value, TITLE_MAX),
            // The summary may not repeat the title (quality rule), so it says where it came from.
            summary: clip(
                &format!("{} noted in a session: {}", self.kind.as_str(), self.value),
                SUMMARY_MAX,
            ),
            // Provenance only; no cause is invented.
            reason: if self.reason.chars().count() < 10 {
                "Distilled from a captured session note.".to_owned()
            } else {
                self.reason
            },
            fields: Some(Value::Object(self.fields)),
            evidence: vec![self.event_id],
            supersedes: None,
            contradicts: None,
            scope: None,
        };
        (input, self.files)
    }
}

fn clip(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// A command, heading, quote or option line. A pasted brief (an agent preamble, a README) is full
/// of them, and their "blocked" or "should" is not the person's own open item.
fn is_boilerplate(line: &str) -> bool {
    line.starts_with(['#', '$', '>'])
        || line
            .split(' ')
            .any(|word| word.len() > 2 && word.starts_with("--"))
}

fn has_marker(lower: &str, markers: &[&str]) -> bool {
    markers.iter().any(|marker| lower.contains(marker))
}

/// `next:` or `todo:` at the start of the line, after an optional `- ` or `* `.
fn is_next_step(line: &str) -> bool {
    let line = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line);
    let lower = line.to_ascii_lowercase();
    lower.starts_with("next:") || lower.starts_with("todo:")
}

fn handoff_summary(files: usize, outside: u32, open: usize, when: &str) -> String {
    let edited = if outside > 0 {
        format!("{files} files edited ({outside} outside the project)")
    } else {
        format!("{files} files edited")
    };
    format!("{edited}; {open} open items; last activity {when}.")
}

fn push_unique(list: &mut Vec<String>, value: &str) -> bool {
    let value = clip(value, LIST_ITEM_MAX);
    if value.is_empty() || list.len() >= LIST_MAX_ITEMS || list.contains(&value) {
        return false;
    }
    list.push(value);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureKind, WorkContext};
    use crate::memory::validate::validate_quality;
    use crate::memory::{Fields, Kind};

    fn ob(
        id: &str,
        kind: CaptureKind,
        content: &str,
        ok: Option<bool>,
        files: &[&str],
    ) -> Observation {
        let command =
            (kind == CaptureKind::Command).then(|| content.lines().next().unwrap_or("").to_owned());
        Observation {
            event_id: id.into(),
            session_id: "s1".into(),
            content: content.into(),
            role: kind.role().into(),
            source: kind.as_str().into(),
            kind,
            ok,
            content_hash: String::new(),
            context: WorkContext {
                files: files.iter().map(|f| (*f).to_owned()).collect(),
                commands: command.into_iter().collect(),
                branch: Some("feat/x".into()),
                repository: Some("github.com/acme/shop".into()),
                ..WorkContext::default()
            },
            captured_at: 1_790_000_000,
            expires_at: 0,
        }
    }

    fn prompt(id: &str, text: &str) -> Observation {
        ob(id, CaptureKind::Prompt, text, None, &[])
    }

    fn fail(id: &str, cmd: &str, error: &str) -> Observation {
        ob(
            id,
            CaptureKind::Command,
            &format!("{cmd}\nexit 101: {error}"),
            Some(false),
            &[],
        )
    }

    fn pass(id: &str, cmd: &str) -> Observation {
        ob(id, CaptureKind::Command, cmd, Some(true), &[])
    }

    fn edit(id: &str, file: &str) -> Observation {
        ob(
            id,
            CaptureKind::Edit,
            &format!("Edit {file}\nx"),
            None,
            &[file],
        )
    }

    fn quality(input: &SaveInput) {
        let fields = Fields::parse(input.kind, input.fields.clone()).expect("fields");
        assert_eq!(validate_quality(input, &fields), Ok(()), "{input:?}");
    }

    #[test]
    fn typed_notes_take_following_metadata_only() {
        let d = distill(&[ob(
            "e1",
            CaptureKind::Assistant,
            "Decision: use pgvector\nReason: one database\nAlternative: qdrant\nSome prose\nRule: no mocks\nFix: ignored for a rule",
            None,
            &[],
        )]);
        assert_eq!(d.notes.len(), 2);
        let decision = &d.notes[0].input;
        assert_eq!(
            (decision.kind, decision.reason.as_str()),
            (Kind::Decision, "one database")
        );
        assert_eq!(
            decision.fields.as_ref().unwrap()["alternatives"][0],
            "qdrant"
        );
        assert_eq!(decision.evidence, vec!["e1".to_owned()]);
        assert_eq!(decision.scope, None);
        assert!(d.notes[1].input.reason.starts_with("Distilled"));
    }

    #[test]
    fn test_commands_and_typed_lines_are_recognised() {
        for yes in [
            "cargo test --workspace",
            "bun test",
            "bunx vitest run",
            "pnpm test",
            "npm test",
            "yarn test",
        ] {
            assert!(is_test_command(yes), "{yes}");
        }
        for no in [
            "bun install",
            "bunx prettier .",
            "npm install",
            "cargo build",
            "testing",
        ] {
            assert!(!is_test_command(no), "{no}");
        }
        assert!(has_typed_line("some prose\n  Decision: use pgvector"));
        assert!(has_typed_line("gotcha: lock timeouts"));
        assert!(!has_typed_line("Decision:\nprose, no value"));
        assert!(!has_typed_line("Note: not a typed label"));
    }

    #[test]
    fn correction_patterns() {
        for yes in [
            "No, never use npm here",
            "always run bun test first",
            "don't touch the migrations",
        ] {
            assert!(is_correction(yes), "{yes}");
        }
        for no in [
            "can you always-on the logs?",
            "I never saw this error before?",
            "Decision: use bun",
        ] {
            assert!(!is_correction(no), "{no}");
        }
        let fenced = "```\nnever use npm\n```\n# never do this\n";
        assert_eq!(
            first_correction(fenced),
            None,
            "fences and headings are not corrections"
        );
        let after = "```sh\ndon't touch the migrations\n```\nalways run bun test first\n";
        assert_eq!(
            first_correction(after).as_deref(),
            Some("always run bun test first")
        );
    }

    #[test]
    fn typed_lines_in_tool_captures_are_ignored() {
        let d = distill(&[ob(
            "e1",
            CaptureKind::Command,
            "echo\nDecision: from a tool",
            None,
            &[],
        )]);
        assert!(d.notes.is_empty());
    }

    #[test]
    fn distill_fail_fix_gotcha() {
        let d = distill(&[
            fail(
                "c1",
                "cargo test --workspace",
                "error[E0308]: mismatched types in src/billing/money.rs",
            ),
            edit("e1", "src/billing/money.rs"),
            pass("c2", "cargo test"),
        ]);
        let gotcha = d
            .notes
            .iter()
            .find(|n| n.input.kind == Kind::Gotcha)
            .expect("gotcha");
        let input = &gotcha.input;
        assert_eq!(input.title, "`cargo test` failed until money.rs changed");
        assert_eq!(
            input.summary,
            "error[E0308]: mismatched types in src/billing/money.rs"
        );
        let fields = input.fields.as_ref().unwrap();
        assert_eq!(fields["fix"], "Edited src/billing/money.rs");
        assert_eq!(
            fields["symptom"],
            "error[E0308]: mismatched types in src/billing/money.rs"
        );
        assert!(fields.get("cause").is_none(), "no cause is invented");
        assert_eq!(input.evidence, ["c1", "e1", "c2"]);
        assert_eq!(gotcha.files, ["src/billing/money.rs"]);
        // a short error line falls back to the generated summary
        let d = distill(&[
            fail("c1", "go test ./...", "boom"),
            edit("e1", "a.go"),
            pass("c2", "go test"),
        ]);
        let g = d
            .notes
            .iter()
            .find(|n| n.input.kind == Kind::Gotcha)
            .unwrap();
        assert_eq!(
            g.input.summary,
            "`go test` failed, then passed after editing a.go."
        );
        assert_eq!(g.input.fields.as_ref().unwrap()["symptom"], "boom");
    }

    #[test]
    fn distill_no_gotcha_without_edit_between() {
        let none = |obs: &[Observation]| {
            distill(obs)
                .notes
                .iter()
                .all(|n| n.input.kind != Kind::Gotcha)
        };
        assert!(none(&[
            fail("c1", "cargo test", "error: nope nope nope nope"),
            pass("c2", "cargo test")
        ]));
        // the edit comes before the failure
        assert!(none(&[
            edit("e1", "a.rs"),
            fail("c1", "cargo test", "x"),
            pass("c2", "cargo test")
        ]));
        // another command passed, not the same key
        assert!(none(&[
            fail("c1", "cargo test", "x"),
            edit("e1", "a.rs"),
            pass("c2", "cargo build")
        ]));
    }

    #[test]
    fn distill_ignores_ok_none() {
        let unknown = ob("c1", CaptureKind::Command, "cargo test\nfailed", None, &[]);
        let d = distill(&[
            unknown.clone(),
            edit("e1", "a.rs"),
            ob("c2", CaptureKind::Command, "cargo test", None, &[]),
        ]);
        assert!(d.notes.is_empty(), "{d:?}");
    }

    #[test]
    fn distill_test_command_fact_dedupes() {
        let d = distill(&[
            pass("c1", "cargo test --workspace -q"),
            pass("c2", "cargo test --workspace -q"),
        ]);
        let facts: Vec<_> = d
            .notes
            .iter()
            .filter(|n| n.input.kind == Kind::Fact)
            .collect();
        assert_eq!(facts.len(), 1);
        assert_eq!(
            facts[0].input.title,
            "Tests run with `cargo test --workspace`"
        );
        assert_eq!(
            facts[0].input.fields.as_ref().unwrap()["source"],
            "session capture"
        );
        assert_eq!(facts[0].input.evidence, ["c1"]);
        // identity: the same session distilled twice gives the same fact input
        let again = distill(&[pass("c1", "cargo test --workspace -q")]);
        assert_eq!(again.notes[0].input.title, facts[0].input.title);
        assert!(distill(&[pass("c1", "cargo build")]).notes.is_empty());
        assert!(
            distill(&[ob(
                "c1",
                CaptureKind::Command,
                "cargo test\nexit 1: x",
                Some(false),
                &[]
            )])
            .notes
            .is_empty()
        );
    }

    #[test]
    fn next_steps_need_explicit_marker() {
        let vague = distill(&[prompt(
            "p1",
            "we should maybe adjust the roadmap, need to think",
        )]);
        let steps = vague
            .handoff
            .and_then(|handoff| handoff.fields)
            .map(|fields| fields["next_steps"].clone())
            .unwrap_or(json!([]));
        assert_eq!(steps, json!([]));

        let marked = distill(&[prompt("p2", "next: wire the web report")]);
        assert_eq!(
            marked.handoff.expect("next").fields.unwrap()["next_steps"],
            json!(["next: wire the web report"])
        );
        let todo = distill(&[prompt("p3", "TODO: fix flaky test")]);
        assert_eq!(
            todo.handoff.expect("todo").fields.unwrap()["next_steps"],
            json!(["TODO: fix flaky test"])
        );
    }

    #[test]
    fn handoff_ignores_pasted_commands_headings_and_code() {
        let brief = "# Blocked, and the coordinator must act before you can continue:\n\
            orca orchestration send --from term_1 --subject \"Blocked: x\" --task-id t\n\
            > blocked on the reviewer\n\
            $ cargo test --workspace # remaining\n\
            ```sh\n\
            todo: inside a fence\n\
            ```\n\
            TODO: wire the worker";
        let d = distill(&[prompt("p1", brief), edit("e1", "src/a.rs")]);
        let fields = d.handoff.expect("handoff").fields.unwrap();
        assert_eq!(
            fields["next_steps"],
            serde_json::json!(["TODO: wire the worker"])
        );
        assert_eq!(
            fields["open_questions"],
            serde_json::json!(["TODO: wire the worker"])
        );
    }

    #[test]
    fn distill_handoff_title_has_no_session_id() {
        let d = distill(&[
            prompt("p1", "TODO: wire the worker"),
            edit("e1", "src/a.rs"),
            fail("c1", "cargo test", "error: still broken"),
        ]);
        let h = d.handoff.expect("handoff");
        assert_eq!(h.title, "Handoff: feat/x");
        assert_eq!(
            h.summary,
            "1 files edited; 2 open items; last activity 2026-09-21."
        );
        let fields = h.fields.unwrap();
        assert_eq!(fields["next_steps"][0], "TODO: wire the worker");
        assert_eq!(
            fields["open_questions"][1],
            "`cargo test` still failing: error: still broken"
        );
        assert_eq!(fields["files"][0], "src/a.rs");
        // no branch: repository basename, then "workspace"
        let mut o = edit("e1", "a.rs");
        o.context.branch = None;
        assert_eq!(
            distill(&[o.clone()]).handoff.unwrap().title,
            "Handoff: shop"
        );
        o.context.repository = None;
        assert_eq!(distill(&[o]).handoff.unwrap().title, "Handoff: workspace");
        // a failure that later passed is not open; nothing to say → none
        assert!(
            distill(&[prompt("p1", "hello"), pass("c1", "ls -l")])
                .handoff
                .is_none()
        );
        let d = distill(&[fail("c1", "cargo test", "x"), pass("c2", "cargo test")]);
        assert!(d.handoff.is_none());
    }

    #[test]
    fn handoff_id_per_checkout() {
        let ctx = |branch: &str, wt: &str| WorkContext {
            project_id: Some("proj".into()),
            repository: Some("r".into()),
            branch: Some(branch.into()),
            worktree: Some(wt.into()),
            ..WorkContext::default()
        };
        let id = handoff_id("p", &ctx("main", "wt-1"));
        assert_eq!(id, handoff_id("p", &ctx("main", "wt-1")));
        assert!(id.starts_with("hnd_") && id.len() == 36);
        for other in [
            handoff_id("q", &ctx("main", "wt-1")),
            handoff_id("p", &ctx("dev", "wt-1")),
            handoff_id("p", &ctx("main", "wt-2")),
            handoff_id("p", &WorkContext::default()),
        ] {
            assert_ne!(id, other);
        }
        // the project id wins over the repository name
        let mut c = ctx("main", "wt-1");
        c.repository = Some("other".into());
        assert_eq!(id, handoff_id("p", &c));
    }

    #[test]
    fn system_outputs_pass_quality() {
        let d = distill(&[
            prompt(
                "p1",
                "TODO: wire the worker\nDecision: use pgvector\nReason: one database\nGotcha: lock timeouts under load\nFix: retry\nProcedure: ship a release\nStep: build\nStep: push\nGotcha: lock\nProcedure: one step only\nStep: build",
            ),
            fail("c1", "cargo test", "boom"),
            edit(
                "e1",
                "src/very/long/path/to/a/module/file_with_a_long_name.rs",
            ),
            pass("c2", "cargo test"),
        ]);
        let mut inputs: Vec<&SaveInput> = d.notes.iter().map(|n| &n.input).collect();
        inputs.extend(d.handoff.as_ref());
        assert!(
            inputs.iter().any(|i| i.kind == Kind::Gotcha)
                && inputs.iter().any(|i| i.kind == Kind::Handoff)
        );
        assert!(inputs.iter().any(|i| i.kind == Kind::Fact));
        // typed lines that cannot pass (gotcha `lock`, a one-step procedure) are dropped, not saved
        assert_eq!(
            d.notes.iter().filter(|n| n.input.title == "lock").count(),
            0
        );
        assert_eq!(
            d.notes
                .iter()
                .filter(|n| n.input.title == "one step only")
                .count(),
            0
        );
        for input in inputs {
            quality(input);
        }
    }

    #[test]
    fn digest_lists_files_and_test_results() {
        let prompt_text = "zephyr-quilt: do not copy this prompt into the digest";
        let d = distill(&[
            prompt("p1", prompt_text),
            fail(
                "c1",
                "cargo test --workspace",
                "error[E0308]: mismatched types in billing",
            ),
            edit("e1", "src/ui/LedControls.tsx"),
            edit("e2", "src/panel/removed.tsx"),
            pass("c2", "cargo test"),
            pass("c3", "cargo build"),
        ]);
        let digest = d.digest.expect("digest");
        assert_eq!(digest.kind, Kind::Note);
        assert_eq!(digest.title, "Session 2026-09-21 on feat/x");
        assert_eq!(
            digest.summary,
            "Changed src/ui/LedControls.tsx, src/panel/removed.tsx. Tests: cargo test pass."
        );
        assert!(!digest.summary.contains("zephyr-quilt"), "{digest:?}");
        assert!(!digest.summary.contains("mismatched"), "{digest:?}");
        assert!(!digest.summary.contains("cargo build"), "{digest:?}");
        assert!(!digest.summary.contains("fail"), "{digest:?}");
        assert_eq!(digest.reason, "Derived from the captures of one session.");
        assert_eq!(
            digest.evidence,
            [
                "src/ui/LedControls.tsx",
                "src/panel/removed.tsx",
                "cargo test",
            ]
        );
        let mut many = vec![pass("t1", "cargo test")];
        for index in 0..8 {
            many.push(edit(&format!("m{index}"), &format!("f{index}.rs")));
        }
        let summary = distill(&many).digest.expect("long digest").summary;
        assert_eq!(
            summary,
            "Changed f0.rs, f1.rs, f2.rs, f3.rs, f4.rs, f5.rs, +2 more. Tests: cargo test pass."
        );
        assert!(!summary.contains("zephyr-quilt"));
    }

    #[test]
    fn digest_omits_tests_when_there_are_none() {
        let summary = distill(&[edit("e1", "src/a.rs")])
            .digest
            .expect("digest")
            .summary;
        assert_eq!(summary, "Changed src/a.rs.");
        assert!(!summary.contains("Tests:"), "{summary}");
    }

    #[test]
    fn no_digest_without_changed_files() {
        assert!(
            distill(&[prompt("p1", "TODO: later"), pass("c1", "cargo test")])
                .digest
                .is_none()
        );
        assert!(
            distill(&[fail("c1", "cargo test", "boom boom boom boom")])
                .digest
                .is_none()
        );
    }
}
