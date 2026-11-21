//! `cairn`: the agent CLI. Every command is one REST call authenticated with the device token
//! in the selected config file. Humans use the web console.

use cairn_client::tools::{self, Tool};
use cairn_client::{
    Client, Config, Error, doctor, hook, mcp, project, selected_config_path, setup, update,
    warn_ignored_legacy_config,
};
use serde_json::{Map, Value, json};
use std::env;
use std::io::{self, BufRead, IsTerminal};
use std::path::PathBuf;
use std::process;

const USAGE: &str = "Usage:
  cairn setup <server-url> [--agent claude|codex|opencode] [--remove]
  cairn init [name]             (create or join; write .cairn/project.json)
  cairn init --rename <name>    (owner only; update server and project file)
  cairn doctor                  (check config, server, device, project, agents)
  cairn mcp
  cairn update [--check]        (install the latest release; --check only reports)
  cairn --version
  cairn search <query> [--kind <kind>] [--limit <n>]
  cairn get <id>...
  cairn save [JSON-record|-]    (SaveInput JSON; stdin when absent or -)
  cairn feedback <id> helpful|wrong
  cairn forget <id>
Create the device token in the web console; it is an agent credential. Account and device
management stay in the web console.";

fn print_usage() {
    println!("{USAGE}");
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
            "init" | "setup" | "mcp" | "update" | "version" | "--version" | "doctor" | "hook"
        ) || tools::find(name).is_some()
    };
    if matches!(args, [flag] if flag == "--help" || flag == "-h")
        || matches!(args, [help] if help == "help")
        || matches!(args, [help, name] if help == "help" && known(name))
        || matches!(args, [name, flag] if known(name) && (flag == "--help" || flag == "-h"))
    {
        return Ok(true);
    }
    // `hook` parses its own options (they come from the host config); so does `search`.
    if matches!(args.first().map(String::as_str), Some("hook" | "search")) {
        return Ok(false);
    }
    for (index, value) in args.iter().enumerate() {
        let allowed = (index == 0 && value == "--version")
            || (index == 1 && args[0] == "update" && value == "--check")
            || (index == 1 && args[0] == "init" && value == "--rename")
            || (args[0] == "setup"
                && ((index == 2 && value == "--agent") || (index == 4 && value == "--remove")));
        if value.starts_with('-') && value != "-" && !allowed {
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
        Some("doctor") if args.len() == 1 => doctor_command(),
        Some("hook") => match hook::run(&args[1..]) {
            Ok(code) => process::exit(code),
            Err(error) => Err(error),
        },
        Some("version" | "--version") if args.len() == 1 => {
            println!("cairn {}", update::VERSION);
            Ok(())
        }
        Some("update") => match &args[1..] {
            [] => update::update(false),
            [flag] if flag == "--check" => update::update(true),
            _ => usage(),
        },
        Some(name) => match tools::find(name).filter(|tool| tool.cli == name) {
            Some(tool) => run(tool, &args[1..]),
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
        Some("mcp" | "update" | "version" | "--version" | "doctor" | "hook")
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
    if agent.is_some_and(|a| !setup::AGENTS.contains(&a)) {
        return Err(Error::Input(
            "agent must be claude, codex or opencode".into(),
        ));
    }
    if remove {
        println!(
            "{}",
            setup::configure(agent.expect("remove has an agent"), true)?
        );
        return Ok(());
    }
    warn_ignored_legacy_config();
    let config = Config::new(url, &read_token()?)?;
    let path = selected_config_path()?;
    config.save(&path)?;
    println!("Cairn configured for {} ({})", config.url(), path.display());
    setup::retire_python(path.parent().unwrap_or(std::path::Path::new(".")));
    if let Some(agent) = agent {
        println!("{}", setup::configure(agent, false)?);
        if agent == "codex" {
            println!("Review and trust the installed hooks with /hooks in Codex before they run.");
        }
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
        // `.` and `..` survive percent-encoding and would climb out of the route.
        if !file
            .project_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(Error::Input(
                "project id in .cairn/project.json is invalid".to_owned(),
            ));
        }
        let path = format!("/cairn-memory/projects/{}/rename", file.project_id);
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

fn doctor_command() -> Result<(), Error> {
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| Error::Config("no home directory (set HOME)".to_owned()))?;
    if doctor::print(&doctor::run(&home)) {
        process::exit(1);
    }
    Ok(())
}

fn run(tool: &Tool, args: &[String]) -> Result<(), Error> {
    let arguments = tool_arguments(tool, args)?;
    let client = Client::new(Config::load()?);
    println!("{}", tools::call(&client, tool, arguments)?);
    Ok(())
}

/// Maps the verb's positionals and flags onto the tool's JSON arguments.
fn tool_arguments(tool: &Tool, args: &[String]) -> Result<Map<String, Value>, Error> {
    let mut arguments = Map::new();
    match tool.cli {
        "search" => {
            let mut words = Vec::new();
            let mut rest = args.iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--kind" => {
                        let kind = rest
                            .next()
                            .ok_or_else(|| Error::Input("--kind needs a value".to_owned()))?;
                        arguments.insert("kind".to_owned(), json!(kind));
                    }
                    "--limit" => {
                        let limit = rest
                            .next()
                            .and_then(|value| value.parse::<u64>().ok())
                            .ok_or_else(|| {
                                Error::Input("--limit must be a positive integer".to_owned())
                            })?;
                        arguments.insert("limit".to_owned(), json!(limit));
                    }
                    flag if flag.starts_with("--") => {
                        return Err(Error::Input(format!(
                            "unexpected option {flag:?}; run `cairn --help`"
                        )));
                    }
                    word => words.push(word),
                }
            }
            if !words.is_empty() {
                arguments.insert("query".to_owned(), json!(words.join(" ")));
            }
        }
        "get" => {
            arguments.insert("ids".to_owned(), json!(args));
        }
        "save" => {
            let raw = match args {
                [] => read_stdin()?,
                [dash] if dash == "-" => read_stdin()?,
                [raw] => raw.clone(),
                _ => {
                    return Err(Error::Input(
                        "save takes one JSON record, - or stdin".to_owned(),
                    ));
                }
            };
            return serde_json::from_str::<Value>(&raw)
                .map_err(|e| Error::Input(format!("memory record is not valid JSON: {e}")))?
                .as_object()
                .cloned()
                .ok_or_else(|| Error::Input("memory record must be a JSON object".to_owned()));
        }
        "feedback" => {
            if let [id, vote] = args {
                arguments.insert("id".to_owned(), json!(id));
                arguments.insert("vote".to_owned(), json!(vote));
            } else {
                usage();
            }
        }
        _ => {
            if let [id] = args {
                arguments.insert("id".to_owned(), json!(id));
            } else {
                usage();
            }
        }
    }
    Ok(arguments)
}

fn read_stdin() -> Result<String, Error> {
    let mut raw = String::new();
    io::Read::read_to_string(&mut io::stdin(), &mut raw)
        .map_err(|e| Error::Input(e.to_string()))?;
    Ok(raw)
}

#[cfg(test)]
#[path = "cairn/tests.rs"]
mod tests;
