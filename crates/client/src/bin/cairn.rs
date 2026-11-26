//! `cairn`: the agent CLI. REST calls authenticate with the device token in the selected
//! config file. Humans manage the server through the web console.

use cairn_client::tools::{self, Tool};
use cairn_client::{
    Client, Config, Error, claude_memory, doctor, fixes, hook, mcp, project, selected_config_path,
    setup, update, warn_ignored_legacy_config,
};
use serde_json::{Map, Value, json};
use std::env;
use std::io::{self, BufRead, IsTerminal};
use std::path::PathBuf;
use std::process;

const USAGE: &str = "Usage:
  cairn setup <server-url> [--agent claude|codex|opencode|grok|antigravity|hermes] [--remove|--keep-native-memory|--gateway]
  cairn setup <agent> [--remove|--gateway]
  cairn init [name] [--yes]     (create or join; write .cairn/project.json; --yes re-registers an id the server lost)
  cairn import claude-memory [--yes]
  cairn fixes [apply [fx_id]]  (list or apply this device's fixes)
  cairn doctor                  (check config, server, device, project, agents)
  cairn mcp
  cairn update [--check]        (install the latest release; --check only reports)
  cairn --version
  cairn search <query> [--kind <kind>] [--limit <n>]
  cairn get <id>... [--detail]
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
            "init"
                | "setup"
                | "import"
                | "mcp"
                | "update"
                | "version"
                | "--version"
                | "doctor"
                | "hook"
                | "fixes"
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
            || (args[0] == "init" && value == "--yes")
            || (args[0] == "get" && index + 1 == args.len() && value == "--detail")
            || (index == 2 && args[0] == "import" && value == "--yes")
            || (args[0] == "setup"
                && ((index == 2 && value == "--agent")
                    || (index == 2
                        && matches!(value.as_str(), "--remove" | "--gateway")
                        && setup::AGENTS.contains(&args[1].as_str()))
                    || (index == 4
                        && matches!(
                            value.as_str(),
                            "--remove" | "--keep-native-memory" | "--gateway"
                        ))));
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
    let pre_tool_use = args.first().is_some_and(|arg| arg == "hook")
        && args.get(1).is_some_and(|arg| arg == "PreToolUse");
    if !pre_tool_use && args.first().is_none_or(|arg| arg != "import") {
        update::remove_old_exe();
    }
    if matches!(args.first().map(String::as_str), Some("init" | "doctor")) {
        let _ = fixes::apply_automatically();
    }
    let result = match args.first().map(String::as_str) {
        Some("fixes") => match &args[1..] {
            [] => fixes::list(),
            [apply] if apply == "apply" => fixes::apply(None),
            [apply, id] if apply == "apply" => fixes::apply(Some(id)),
            _ => usage(),
        },
        Some("setup") => setup(&args[1..]),
        Some("init") => init(&args[1..]),
        Some("import") => match &args[1..] {
            [source] if source == "claude-memory" => claude_memory::import(false),
            [source, flag] if source == "claude-memory" && flag == "--yes" => {
                claude_memory::import(true)
            }
            _ => usage(),
        },
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
            [] => update::update(false).map(|_| ()),
            [flag] if flag == "--check" => update::update(true).map(|_| ()),
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
    // Keep protocol, diagnostic and import commands free of background update requests.
    if !matches!(
        args.first().map(String::as_str),
        Some("mcp" | "update" | "version" | "--version" | "doctor" | "hook" | "import")
    ) {
        update::notify();
    }
    if failed {
        process::exit(1);
    }
}

fn setup(args: &[String]) -> Result<(), Error> {
    let (url, agent, option) = match args {
        [agent] if setup::AGENTS.contains(&agent.as_str()) => (None, Some(agent.as_str()), None),
        [agent, flag]
            if matches!(flag.as_str(), "--remove" | "--gateway")
                && setup::AGENTS.contains(&agent.as_str()) =>
        {
            (None, Some(agent.as_str()), Some(flag.as_str()))
        }
        [url] => (Some(url), None, None),
        [url, flag, agent] if flag == "--agent" => (Some(url), Some(agent.as_str()), None),
        [url, flag, agent, option]
            if flag == "--agent"
                && matches!(
                    option.as_str(),
                    "--remove" | "--keep-native-memory" | "--gateway"
                ) =>
        {
            (Some(url), Some(agent.as_str()), Some(option.as_str()))
        }
        _ => usage(),
    };
    if agent.is_some_and(|a| !setup::AGENTS.contains(&a)) {
        return Err(Error::Input(format!(
            "agent must be one of {}",
            setup::AGENTS.join(", ")
        )));
    }
    let keep_native_memory = option == Some("--keep-native-memory");
    if keep_native_memory && agent != Some("claude") {
        return Err(Error::Input(
            "--keep-native-memory applies only to Claude Code".into(),
        ));
    }
    let gateway = option == Some("--gateway");
    if gateway && agent != Some("hermes") {
        return Err(Error::Input(
            "--gateway applies only to Hermes Agent".into(),
        ));
    }
    if option == Some("--remove") {
        println!(
            "{}",
            setup::configure(agent.expect("remove has an agent"), true)?
        );
        return Ok(());
    }
    warn_ignored_legacy_config();
    if let Some(url) = url {
        let path = selected_config_path()?;
        let config = Config::new(url, &read_token()?)?.keeping_nudge_of(&path);
        config.save(&path)?;
        println!("Cairn configured for {} ({})", config.url(), path.display());
        setup::retire_python(path.parent().unwrap_or(std::path::Path::new(".")));
    } else {
        Config::load()?;
    }
    if let Some(agent) = agent {
        let done = if url.is_some() || gateway {
            setup::install(agent, keep_native_memory, gateway)?
        } else {
            setup::configure(agent, false)?
        };
        println!("{done}");
        if agent == "codex" {
            println!("Review and trust the installed hooks with /hooks in Codex before they run.");
        }
    }
    Ok(())
}

/// Creates (or finds) the caller's project for this folder and writes
/// `.cairn/project.json`. Inside git the root is the checkout; otherwise it is the working
/// folder and the repository is `local:<folder name>`. Without a remote or project file,
/// creates a new project. This never stages or commits.
fn init(args: &[String]) -> Result<(), Error> {
    let yes = args.iter().any(|arg| arg == "--yes");
    let args: Vec<String> = args.iter().filter(|arg| *arg != "--yes").cloned().collect();
    let name = match args.as_slice() {
        [] => None,
        [name] => Some(name.trim()),
        _ => usage(),
    };
    if name.is_some_and(str::is_empty) {
        return Err(Error::Input("project name must not be empty".to_owned()));
    }
    let cwd = env::current_dir()
        .map_err(|error| Error::Input(format!("cannot read the current directory: {error}")))?;
    let git_top = project::toplevel(&cwd);
    let top = git_top.clone().unwrap_or(cwd);
    let repository = project::repository_name(&top);
    let existing = project::read_at(&top)?;
    if let (Some(requested), Some(recorded)) = (
        name,
        existing.as_ref().and_then(|file| file.name.as_deref()),
    ) && requested != recorded
    {
        return Err(Error::Input(format!(
            "project file names {recorded:?}; rename it in the web console"
        )));
    }
    let client = Client::new(Config::load()?);
    let mut body = json!({ "repository": repository, "confirm_register": yes });
    if let Some(file) = &existing {
        body["project_id"] = json!(file.project_id);
    }
    if let Some(name) = name.or_else(|| existing.as_ref().and_then(|file| file.name.as_deref())) {
        body["name"] = json!(name);
    }
    let created = client.post("/cairn-memory/projects/init", &body)?;
    let project_id = created["project_id"]
        .as_str()
        .ok_or_else(|| Error::Input("the server sent no project id".to_owned()))?;
    let name = created["name"]
        .as_str()
        .ok_or_else(|| Error::Input("the server sent no project name".to_owned()))?;
    let written = project::write(&top, project_id, &repository, name)?;
    if existing
        .as_ref()
        .and_then(|file| file.name.as_deref())
        .is_some_and(|old| old != name)
    {
        println!(
            "Project renamed to {name:?} in the web; updated .cairn/project.json — commit it."
        );
    }
    if created["created"] == true && existing.is_some() {
        println!(
            "Warning: the server did not know project {project_id} (a wiped or new server). Registered it again with the same id and repository {repository}; you are now its owner. Tell your team if you did not expect this."
        );
    } else if !written {
        println!("already initialized, you are a member");
    }
    let path = top.join(".cairn/project.json");
    println!(
        "Cairn project {project_id} ({repository}) {}: {}",
        if written { "written" } else { "already set" },
        path.display()
    );
    if git_top.is_some() && !project::project_file_tracked(&top) {
        println!(
            "Commit .cairn/project.json so clones and worktrees attach: git add .cairn/project.json && git commit"
        );
    } else if git_top.is_none() {
        println!(
            "Keep .cairn/project.json with this folder; copies of the folder attach to the same project."
        );
    }
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
            let args = if let Some((flag, ids)) = args.split_last()
                && flag == "--detail"
            {
                arguments.insert("detail".to_owned(), json!(true));
                ids
            } else {
                args
            };
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
