//! `cairn`: the agent CLI. Every command is one REST call authenticated with the device token
//! that `cairn setup` stored (or `CAIRN_URL` / `CAIRN_DEVICE_TOKEN`). Humans use the web console.

use cairn_client::tools::{self, Tool};
use cairn_client::{Client, Config, Error, config_path, mcp, project, update, write_private};
use serde_json::{Map, Value, json};
use std::env;
use std::fs;
use std::io::{self, BufRead, IsTerminal};
use std::path::Path;
use std::process;

/// A CLI command: positional `args` fill the named tool arguments in order. With `context`,
/// up to four more positionals (repository, branch, worktree, commit) fill the work context.
struct Command {
    name: &'static str,
    tool: &'static str,
    args: &'static [&'static str],
    required: usize,
    defaults: &'static [(&'static str, &'static str)],
    context: bool,
}

const fn command(
    name: &'static str,
    tool: &'static str,
    args: &'static [&'static str],
    required: usize,
) -> Command {
    Command {
        name,
        tool,
        args,
        required,
        defaults: &[],
        context: false,
    }
}

const COMMANDS: &[Command] = &[
    Command {
        context: true,
        ..command("start", "memory_session_start", &["sessionId"], 1)
    },
    Command {
        defaults: &[("role", "agent"), ("source", "cli")],
        ..command(
            "capture",
            "memory_observe",
            &["sessionId", "eventId", "content", "role", "source"],
            3,
        )
    },
    command("end", "memory_session_end", &["sessionId"], 1),
    Command {
        context: true,
        ..command("search", "memory_search", &["query", "limit"], 1)
    },
    Command {
        context: true,
        ..command("context", "memory_context", &["query", "limit"], 1)
    },
    Command {
        context: true,
        ..command("why-changed", "memory_why_changed", &["query", "limit"], 1)
    },
    Command {
        context: true,
        ..command("resume", "memory_resume", &["limit"], 0)
    },
    Command {
        context: true,
        ..command("unresolved", "memory_unresolved", &["limit"], 0)
    },
    command("forget", "memory_forget", &["id"], 1),
    command("save", "memory_save", &["record"], 0),
    command("lesson", "memory_lesson_save", &["record"], 0),
    command("memories", "memory_list", &[], 0),
    command("memory", "memory_get", &["memoryId"], 1),
    command("feedback", "memory_feedback", &["memoryId", "vote"], 2),
    command(
        "contradict",
        "memory_contradict",
        &["contradictingId", "contradictedId"],
        2,
    ),
    command(
        "supersede",
        "memory_supersede",
        &["successorId", "targetId"],
        2,
    ),
    command("reviews", "memory_similarity_reviews", &[], 0),
    command("review-dismiss", "memory_review_dismiss", &["reviewId"], 1),
    command(
        "review-contradict",
        "memory_review_contradict",
        &["reviewId", "contradictedId"],
        2,
    ),
    command(
        "review-supersede",
        "memory_review_supersede",
        &["reviewId", "successorId"],
        2,
    ),
    command("correct", "memory_correct", &["targetId", "content"], 2),
    command(
        "project-register",
        "memory_project_register",
        &["repository", "name", "defaultBranch"],
        2,
    ),
    command("project-list", "memory_project_list", &[], 0),
    command(
        "project-memories",
        "memory_project_memories",
        &["projectId"],
        1,
    ),
    command(
        "project-freshness",
        "memory_project_freshness",
        &["projectId"],
        1,
    ),
    command(
        "project-members",
        "memory_project_members",
        &["projectId"],
        1,
    ),
    command(
        "project-verify",
        "memory_project_verify",
        &["projectId", "memoryId", "commit"],
        3,
    ),
    command("delete-memory", "memory_delete", &["id"], 1),
    command("export", "memory_export", &["output"], 1),
    command("import", "memory_import", &["input"], 1),
];

const CONTEXT_KEYS: [&str; 4] = ["repository", "branch", "worktree", "commit"];

fn print_usage() {
    println!(
        "Usage:\n\
  cairn setup <server-url> [--agent claude|codex|opencode] [--remove]\n\
  cairn init [name]             (create or join; write .cairn/project.json)\n\
  cairn init --rename <name>    (owner only; update server and project file)\n\
  cairn mcp\n\
  cairn update [--check]        (install the latest release; --check only reports)\n\
  cairn --version\n\
  cairn start <session-id> [repository] [branch] [worktree] [commit]\n\
  cairn capture <session-id> <event-id> <content> [role] [source]\n\
  cairn end <session-id>\n\
  cairn search <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn context <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn why-changed <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn resume [limit] [repository] [branch] [worktree] [commit]\n\
  cairn unresolved [limit] [repository] [branch] [worktree] [commit]\n\
  cairn forget <memory-or-event-id>\n\
  cairn save|lesson [JSON-record] (or stdin)\n\
  cairn memories | memory <memory-id>\n\
  cairn feedback <memory-id> helpful|stale\n\
  cairn contradict <contradicting-id> <contradicted-id>\n\
  cairn supersede <successor-id> <target-id>\n\
  cairn reviews\n\
  cairn review-dismiss <review-id>\n\
  cairn review-contradict <review-id> <contradicted-id>\n\
  cairn review-supersede <review-id> <successor-id>\n\
  cairn correct <memory-or-event-id> <content>\n\
  cairn export <output>\n\
  cairn import <input>\n\
  cairn project-register <repository> <name> [default-branch]\n\
  cairn project-list\n\
  cairn project-members <project-id>\n\
  cairn project-verify <project-id> <memory-id> <commit>\n\
  cairn project-memories <project-id>\n\
  cairn project-freshness <project-id>\n\
  cairn delete-memory <memory-id>\n\
Create the device token in the web console; it is an agent credential. Account and device\n\
management stay in the web console."
    );
}

fn usage() -> ! {
    print_usage();
    process::exit(2);
}

/// Help and option errors are handled before config, stdin, requests or update checks.
fn check_arguments(args: &[String]) -> Result<bool, Error> {
    let known = |name: &str| {
        matches!(
            name,
            "init" | "setup" | "mcp" | "update" | "version" | "--version"
        ) || COMMANDS.iter().any(|command| command.name == name)
    };
    if matches!(args, [flag] if flag == "--help" || flag == "-h")
        || matches!(args, [help] if help == "help")
        || matches!(args, [help, name] if help == "help" && known(name))
        || matches!(args, [name, flag] if known(name) && (flag == "--help" || flag == "-h"))
    {
        return Ok(true);
    }
    for (index, value) in args.iter().enumerate() {
        let allowed = (index == 0 && value == "--version")
            || (index == 1 && args[0] == "update" && value == "--check")
            || (index == 1 && args[0] == "init" && value == "--rename")
            || (args[0] == "setup"
                && ((index == 2 && value == "--agent") || (index == 4 && value == "--remove")));
        if value.starts_with('-') && !allowed {
            return Err(Error::Input(format!(
                "unexpected option {value:?}; run `cairn --help`"
            )));
        }
    }
    Ok(false)
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match check_arguments(&args) {
        Ok(true) => {
            print_usage();
            return;
        }
        Ok(false) => {}
        Err(error) => {
            eprintln!("cairn: {error}");
            process::exit(1);
        }
    }
    update::remove_old_exe();
    let result = match args.first().map(String::as_str) {
        Some("setup") => setup(&args[1..]),
        Some("init") => init(&args[1..]),
        Some("mcp") if args.len() == 1 => Config::load().and_then(mcp::run_stdio),
        Some("version" | "--version") if args.len() == 1 => {
            println!("cairn {}", update::VERSION);
            Ok(())
        }
        Some("update") => match &args[1..] {
            [] => update::update(false),
            [flag] if flag == "--check" => update::update(true),
            _ => usage(),
        },
        Some(name) => match COMMANDS.iter().find(|command| command.name == name) {
            Some(command) => run(command, &args[1..]),
            None => usage(),
        },
        None => usage(),
    };
    let failed = result.is_err();
    if let Err(error) = result {
        eprintln!("cairn: {error}");
    }
    // Never on `mcp` (a protocol stream), `update` (it reports itself) or the version flag.
    if !matches!(
        args.first().map(String::as_str),
        Some("mcp" | "update" | "version" | "--version")
    ) {
        update::notify();
    }
    if failed {
        process::exit(1);
    }
}

fn setup(args: &[String]) -> Result<(), Error> {
    let (url, agent, remove) = match args {
        [url] => (url, None, false),
        [url, flag, agent] if flag == "--agent" => (url, Some(agent.as_str()), false),
        [url, flag, agent, remove] if flag == "--agent" && remove == "--remove" => {
            (url, Some(agent.as_str()), true)
        }
        _ => usage(),
    };
    if agent.is_some_and(|a| !cairn_client::agents::AGENTS.contains(&a)) {
        return Err(Error::Input(
            "agent must be claude, codex or opencode".into(),
        ));
    }
    if remove {
        println!(
            "{}",
            cairn_client::agents::configure(agent.expect("remove has an agent"), true)?
        );
        return Ok(());
    }
    let config = Config::new(url, &read_token()?)?;
    let path = config_path()?;
    config.save(&path)?;
    println!("Cairn configured for {} ({})", config.url(), path.display());
    if let Some(agent) = agent {
        println!("{}", update::install_bundled_hooks()?);
        println!("{}", cairn_client::agents::configure(agent, false)?);
        if agent == "codex" {
            println!("Review and trust the installed hooks with /hooks in Codex before they run.");
        }
        return Ok(());
    }
    // Offline URL-only setup still retains its valid credentials.
    match update::install_hooks() {
        Ok(message) => println!("{message}"),
        Err(error) => eprintln!(
            "cairn: warning: hook scripts were not installed ({error}); `cairn update` retries"
        ),
    }
    Ok(())
}

/// Creates (or finds) the caller's project for this repository and writes
/// `<git toplevel>/.cairn/project.json`, which people commit so teammates join the project.
fn init(args: &[String]) -> Result<(), Error> {
    let (rename, name) = match args {
        [] => (false, None),
        [name] if name != "--rename" => (false, Some(name.trim())),
        [flag, name] if flag == "--rename" => (true, Some(name.trim())),
        _ => usage(),
    };
    if name.is_some_and(str::is_empty) {
        return Err(Error::Input("project name must not be empty".to_owned()));
    }
    let cwd = env::current_dir()
        .map_err(|error| Error::Input(format!("cannot read the current directory: {error}")))?;
    let top = project::toplevel(&cwd)
        .ok_or_else(|| Error::Input("not inside a git repository".to_owned()))?;
    let repository = project::repository_name(&top);
    let existing = project::read(&cwd)?;
    if !rename
        && let (Some(requested), Some(recorded)) = (
            name,
            existing.as_ref().and_then(|file| file.name.as_deref()),
        )
        && requested != recorded
    {
        return Err(Error::Input(format!(
            "project file names {recorded:?}; use `cairn init --rename <name>` as the owner to rename it"
        )));
    }
    let client = Client::new(Config::load()?);
    let created = if rename {
        let file = existing.as_ref().ok_or_else(|| {
            Error::Input("no .cairn/project.json; run `cairn init [name]` first".to_owned())
        })?;
        let path = tools::fill_path(
            "/cairn-memory/projects/{projectId}/rename",
            &mut Map::from_iter([("projectId".to_owned(), json!(file.project_id))]),
        )?;
        client.post(&path, &json!({"name": name}))?
    } else {
        let mut body = json!({ "repository": repository });
        if let Some(file) = &existing {
            body["project_id"] = json!(file.project_id);
        }
        if let Some(name) = name.or_else(|| existing.as_ref().and_then(|file| file.name.as_deref()))
        {
            body["name"] = json!(name);
        }
        client.post("/cairn-memory/projects/init", &body)?
    };
    let project_id = created["project_id"]
        .as_str()
        .ok_or_else(|| Error::Input("the server sent no project id".to_owned()))?;
    let name = created["name"]
        .as_str()
        .ok_or_else(|| Error::Input("the server sent no project name".to_owned()))?;
    let written = project::write(&top, project_id, &repository, name)?;
    if !written {
        println!("already initialized, you are a member");
    }
    let path = top.join(".cairn/project.json");
    println!(
        "Cairn project {project_id} ({repository}) {}: {}",
        if written { "written" } else { "already set" },
        path.display()
    );
    println!("Commit .cairn/project.json so teammates who capture here join the project.");
    Ok(())
}

/// How the token is read: piped, typed without echo, or typed with echo.
#[derive(Debug, PartialEq)]
enum TokenInput {
    Piped,
    Hidden,
    Visible,
}

/// `hide` runs only for a terminal, and a terminal that cannot hide input falls back to visible.
fn token_input(interactive: bool, hide: impl FnOnce() -> bool) -> TokenInput {
    match (interactive, interactive && hide()) {
        (false, _) => TokenInput::Piped,
        (true, true) => TokenInput::Hidden,
        (true, false) => TokenInput::Visible,
    }
}

/// The token never appears in argv or shell history: one line from stdin, typed without
/// echo on a unix terminal. Elsewhere (or without `stty`) it is typed visibly, with a warning.
fn read_token() -> Result<String, Error> {
    let stdin = io::stdin();
    let mode = token_input(stdin.is_terminal(), || set_echo(false));
    match mode {
        TokenInput::Piped => {}
        TokenInput::Hidden => eprint!("Device token (input hidden): "),
        TokenInput::Visible => eprintln!(
            "cairn: warning: token input is visible on this terminal; piping it on stdin is recommended"
        ),
    }
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if mode == TokenInput::Hidden {
        set_echo(true);
        eprintln!();
    }
    read.map_err(|error| Error::Input(format!("cannot read the device token: {error}")))?;
    Ok(line)
}

/// shortcut: `stty` is unix only and may be missing; the caller falls back to visible input.
fn set_echo(on: bool) -> bool {
    cfg!(unix)
        && process::Command::new("stty")
            .arg(if on { "echo" } else { "-echo" })
            .status()
            .is_ok_and(|status| status.success())
}

fn run(command: &Command, args: &[String]) -> Result<(), Error> {
    if args.len() < command.required {
        eprintln!("missing argument for {}", command.name);
        usage();
    }
    let tool = tools::find(command.tool).expect("every command names a tool in the table");
    let arguments = tool_arguments(command, args)?;
    let client = Client::new(Config::load()?);
    let result = if command.name == "export" {
        export(&client, tool, &arguments)?
    } else {
        tools::call(&client, tool, &arguments)?
    };
    if !result.is_null() {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("JSON value serializes")
        );
    }
    Ok(())
}

fn tool_arguments(command: &Command, args: &[String]) -> Result<Map<String, Value>, Error> {
    if matches!(command.name, "save" | "lesson") {
        let raw = match args {
            [] => {
                let mut raw = String::new();
                io::Read::read_to_string(&mut io::stdin(), &mut raw)
                    .map_err(|e| Error::Input(e.to_string()))?;
                raw
            }
            [raw] => raw.clone(),
            _ => {
                return Err(Error::Input(
                    "save/lesson takes one JSON record or stdin".to_owned(),
                ));
            }
        };
        return serde_json::from_str::<Value>(&raw)
            .map_err(|e| Error::Input(format!("memory record is not valid JSON: {e}")))?
            .as_object()
            .cloned()
            .ok_or_else(|| Error::Input("memory record must be a JSON object".to_owned()));
    }
    let present = |index: usize| args.get(index).filter(|value| !value.is_empty());
    let mut arguments = Map::new();
    for (index, key) in command.args.iter().enumerate() {
        if let Some(value) = present(index) {
            arguments.insert((*key).to_owned(), argument_value(key, value)?);
        }
    }
    for (key, value) in command.defaults {
        arguments.entry(*key).or_insert_with(|| {
            if *key == "dryRun" {
                json!(*value == "true")
            } else {
                json!(value)
            }
        });
    }
    if command.context {
        let context: Map<String, Value> = CONTEXT_KEYS
            .iter()
            .enumerate()
            .filter_map(|(offset, key)| {
                present(command.args.len() + offset).map(|value| ((*key).to_owned(), json!(value)))
            })
            .collect();
        if !context.is_empty() {
            arguments.insert("context".to_owned(), Value::Object(context));
        }
    }
    if command.name == "import" {
        let input = arguments
            .remove("input")
            .and_then(|value| value.as_str().map(str::to_owned));
        let raw = fs::read_to_string(input.as_deref().unwrap_or_default())
            .map_err(|error| Error::Input(format!("cannot read import file: {error}")))?;
        let bundle = serde_json::from_str::<Value>(&raw)
            .map_err(|error| Error::Input(format!("import file is not valid JSON: {error}")))?;
        arguments.insert("bundle".to_owned(), bundle);
    }
    Ok(arguments)
}

fn argument_value(key: &str, value: &str) -> Result<Value, Error> {
    if key == "limit" {
        return value
            .parse::<u64>()
            .map(|limit| json!(limit))
            .map_err(|_| Error::Input("limit must be a positive integer".to_owned()));
    }
    Ok(json!(value))
}

fn export(client: &Client, tool: &Tool, arguments: &Map<String, Value>) -> Result<Value, Error> {
    let output = arguments
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let bundle = tools::call(client, tool, &Map::new())?;
    let raw = serde_json::to_string(&bundle).expect("JSON value serializes");
    write_private(Path::new(output), raw.as_bytes())
        .map_err(|error| Error::Input(format!("cannot write {output}: {error}")))?;
    Ok(Value::Null)
}

#[cfg(test)]
#[path = "cairn/tests.rs"]
mod tests;
