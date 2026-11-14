//! `cairn`: the agent CLI. Every command is one REST call authenticated with the device token
//! that `cairn setup` stored (or `CAIRN_URL` / `CAIRN_DEVICE_TOKEN`). Humans use the web console.

use cairn_client::tools::{self, Tool};
use cairn_client::{Client, Config, Error, config_path, mcp, write_private};
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
    command("lesson", "memory_lesson_save", &["content"], 1),
    command("correct", "memory_correct", &["targetId", "content"], 2),
    command(
        "project-register",
        "memory_project_register",
        &["repository", "name", "defaultBranch"],
        2,
    ),
    command("project-list", "memory_project_list", &[], 0),
    command(
        "promote",
        "memory_promote",
        &["projectId", "memoryId", "reason"],
        3,
    ),
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
        "project-invite",
        "memory_project_invite",
        &["projectId", "email"],
        2,
    ),
    command(
        "project-share",
        "memory_project_share",
        &["projectId", "memoryId", "email"],
        3,
    ),
    command("project-shared", "memory_project_shared", &["projectId"], 1),
    command(
        "project-revoke",
        "memory_project_revoke",
        &["projectId", "memoryId", "email"],
        3,
    ),
    command(
        "project-team-create",
        "memory_project_team_create",
        &["projectId", "name"],
        2,
    ),
    command("project-team", "memory_project_team", &["projectId"], 1),
    command(
        "project-team-add",
        "memory_project_team_add",
        &["projectId", "email"],
        2,
    ),
    command("delete-memory", "memory_delete", &["id"], 1),
    command("export", "memory_export", &["output"], 1),
    command("import", "memory_import", &["input"], 1),
];

const CONTEXT_KEYS: [&str; 4] = ["repository", "branch", "worktree", "commit"];

fn usage() -> ! {
    eprintln!(
        "Usage:\n\
  cairn setup <server-url>      (reads the device token from stdin)\n\
  cairn mcp\n\
  cairn start <session-id> [repository] [branch] [worktree] [commit]\n\
  cairn capture <session-id> <event-id> <content> [role] [source]\n\
  cairn end <session-id>\n\
  cairn search <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn context <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn why-changed <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn resume [limit] [repository] [branch] [worktree] [commit]\n\
  cairn unresolved [limit] [repository] [branch] [worktree] [commit]\n\
  cairn forget <memory-or-event-id>\n\
  cairn lesson <content>\n\
  cairn correct <memory-or-event-id> <content>\n\
  cairn export <output>\n\
  cairn import <input>\n\
  cairn project-register <repository> <name> [default-branch]\n\
  cairn project-list\n\
  cairn promote <project-id> <memory-id> <reason>\n\
  cairn project-memories <project-id>\n\
  cairn project-freshness <project-id>\n\
  cairn project-invite <project-id> <email>\n\
  cairn project-share <project-id> <memory-id> <email>\n\
  cairn project-shared <project-id>\n\
  cairn project-revoke <project-id> <memory-id> <email>\n\
  cairn project-team-create <project-id> <name>\n\
  cairn project-team <project-id>\n\
  cairn project-team-add <project-id> <email>\n\
  cairn delete-memory <memory-id>\n\
Create the device token in the web console; it is an agent credential. Account and device\n\
management stay in the web console."
    );
    process::exit(2);
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("setup") => setup(&args[1..]),
        Some("mcp") => Config::load().and_then(mcp::run_stdio),
        Some(name) => match COMMANDS.iter().find(|command| command.name == name) {
            Some(command) => run(command, &args[1..]),
            None => usage(),
        },
        None => usage(),
    };
    if let Err(error) = result {
        eprintln!("cairn: {error}");
        process::exit(1);
    }
}

fn setup(args: &[String]) -> Result<(), Error> {
    let [url] = args else { usage() };
    let config = Config::new(url, &read_token()?)?;
    let path = config_path()?;
    config.save(&path)?;
    println!("Cairn configured for {} ({})", config.url(), path.display());
    Ok(())
}

/// The token never appears in argv or shell history: one line from stdin, typed without
/// echo on a terminal.
fn read_token() -> Result<String, Error> {
    let stdin = io::stdin();
    let interactive = stdin.is_terminal();
    if interactive {
        // shortcut: `stty` hides the input, so a terminal without it must pipe the token.
        set_echo(false)?;
        eprint!("Device token (input hidden): ");
    }
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if interactive {
        let _ = set_echo(true);
        eprintln!();
    }
    read.map_err(|error| Error::Input(format!("cannot read the device token: {error}")))?;
    Ok(line)
}

fn set_echo(on: bool) -> Result<(), Error> {
    let status = process::Command::new("stty")
        .arg(if on { "echo" } else { "-echo" })
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        _ => Err(Error::Input(
            "cannot hide terminal input; pipe the token on stdin instead".to_owned(),
        )),
    }
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
    let present = |index: usize| args.get(index).filter(|value| !value.is_empty());
    let mut arguments = Map::new();
    for (index, key) in command.args.iter().enumerate() {
        if let Some(value) = present(index) {
            arguments.insert((*key).to_owned(), argument_value(key, value)?);
        }
    }
    for (key, value) in command.defaults {
        arguments.entry(*key).or_insert_with(|| json!(value));
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
