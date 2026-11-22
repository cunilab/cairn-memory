//! Which tool calls are read-only and so never captured. A port of the Python hook's
//! `shell_command_is_readonly`, `READ_ONLY_TOOLS`, `READ_ONLY_ARGV`, `GIT_READ`, `SAFE_FLAGS`,
//! `GIT_SAFE_FLAGS` and `canonical_tool` (same sets, same rules).

const READ_ONLY_TOOLS: [&str; 9] = [
    "Read",
    "read_file",
    "Grep",
    "Glob",
    "LS",
    "WebFetch",
    "WebSearch",
    "NotebookRead",
    "list_dir",
];
const READ_ONLY_ARGV: [&str; 25] = [
    "ls", "cat", "head", "tail", "wc", "pwd", "file", "stat", "which", "whoami", "date", "true",
    "false", "echo", "printf", "rg", "grep", "egrep", "fgrep", "nl", "basename", "dirname",
    "realpath", "readlink", "test",
];
const GIT_READ: [&str; 8] = [
    "status",
    "diff",
    "log",
    "show",
    "rev-parse",
    "blame",
    "ls-files",
    "describe",
];

/// Display-only options per command; any other `-x` argument makes the command a capture
/// (`rg --pre`, `git --ext-diff`, `date -s`).
fn safe_flags(argv0: &str) -> &'static [&'static str] {
    match argv0 {
        "cat" | "head" | "tail" => &["-n"],
        "wc" => &["-l", "-w", "-c"],
        "rg" => &[
            "-i",
            "-n",
            "-w",
            "-F",
            "-l",
            "-S",
            "--ignore-case",
            "--line-number",
            "--fixed-strings",
            "--smart-case",
        ],
        "grep" => &[
            "-i",
            "-n",
            "-w",
            "-F",
            "-l",
            "--ignore-case",
            "--line-number",
            "--fixed-strings",
        ],
        "egrep" | "fgrep" => &["-i", "-n", "-w", "-l"],
        _ => &[],
    }
}

fn git_safe_flags(subcommand: &str) -> &'static [&'static str] {
    match subcommand {
        "status" => &["-s", "-sb", "--short", "--branch", "-b"],
        "diff" => &["--stat", "--name-only", "--name-status", "--numstat"],
        "log" => &["--oneline", "--stat", "--name-only", "-1"],
        "show" => &["--stat", "--name-only", "--oneline"],
        "rev-parse" => &["--abbrev-ref", "--show-toplevel", "--verify"],
        "describe" => &["--tags", "--always"],
        _ => &[],
    }
}

fn options_are_safe(parts: &[&str], allowed: &[&str]) -> bool {
    parts
        .iter()
        .all(|part| !part.starts_with('-') || allowed.contains(part))
}

/// Only a single simple command with no shell syntax. Quotes, pipelines and assignments are
/// ambiguous and stay captured.
pub fn shell_command_is_readonly(command: &str) -> bool {
    let text = command.trim();
    const SYNTAX: [&str; 10] = ["&&", "||", ";", "|", ">", "<", "`", "$(", "\n", "&"];
    if text.is_empty()
        || SYNTAX.iter().any(|token| text.contains(token))
        || text.contains(['"', '\''])
    {
        return false;
    }
    let parts: Vec<&str> = text.split_whitespace().collect();
    let argv0 = parts[0];
    if argv0.contains('/') || argv0.starts_with('.') {
        return false;
    }
    if argv0 == "git" {
        return match parts.get(1) {
            Some(sub) if !sub.starts_with('-') && GIT_READ.contains(sub) => {
                options_are_safe(&parts[2..], git_safe_flags(sub))
            }
            _ => false,
        };
    }
    READ_ONLY_ARGV.contains(&argv0) && options_are_safe(&parts[1..], safe_flags(argv0))
}

/// OpenCode uses lower-case tool ids; Claude and Codex use the names above.
pub fn canonical_tool(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "bash" | "shell" | "exec" => "Bash",
        "edit" => "Edit",
        "write" => "Write",
        "multiedit" => "MultiEdit",
        "apply_patch" | "applypatch" => "apply_patch",
        "read" | "read_file" => "Read",
        "grep" => "Grep",
        "glob" => "Glob",
        "list_dir" => "list_dir",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        _ => name,
    }
    .to_owned()
}

pub fn is_read_only_tool(canonical: &str) -> bool {
    READ_ONLY_TOOLS.contains(&canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_table() {
        for command in [
            "ls",
            "git status",
            "git status -sb",
            "git diff --stat",
            "git log --oneline",
            "cat -n README.md",
            "rg -n needle src",
            "grep -i needle file",
            "wc -l file",
            "pwd",
            "echo hi",
            "date",
        ] {
            assert!(shell_command_is_readonly(command), "{command}");
        }
        for command in [
            "",
            "rm -rf x",
            "cargo test",
            "git commit -m x",
            "git --no-pager log",
            "git push",
            "git diff --ext-diff",
            "ls -la",
            "date -s 2020-01-01",
            "rg --pre cmd needle",
            "cat file > out",
            "cat file | wc",
            "ls && rm x",
            "echo $(rm x)",
            "echo 'quoted'",
            "FOO=1 ls",
            "./ls",
            "/bin/ls",
            "cat `x`",
            "ls &",
        ] {
            assert!(!shell_command_is_readonly(command), "{command}");
        }
    }

    #[test]
    fn tool_names_are_canonical() {
        assert_eq!(canonical_tool("bash"), "Bash");
        assert_eq!(canonical_tool("Shell"), "Bash");
        assert_eq!(canonical_tool("multiedit"), "MultiEdit");
        assert_eq!(canonical_tool("read_file"), "Read");
        assert_eq!(canonical_tool("mcp__x__y"), "mcp__x__y");
        assert!(is_read_only_tool(&canonical_tool("grep")));
        assert!(!is_read_only_tool("Edit"));
    }
}
