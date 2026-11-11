use cairn_core::{LifecycleInput, MemoryStore, ObservationInput, RecallFilter, WorkContext};
use std::env;
use std::fs;
use std::process;

fn usage() -> ! {
    eprintln!(
        "Usage:\n\
  cairn init <store>\n\
  cairn start <store> <session-id> [repository] [branch] [worktree] [commit]\n\
  cairn capture <store> <session-id> <event-id> <content> [role] [source]\n\
  cairn end <store> <session-id>\n\
  cairn search <store> <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn context <store> <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn why-changed <store> <query> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn resume <store> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn unresolved <store> [limit] [repository] [branch] [worktree] [commit]\n\
  cairn forget <store> <memory-or-event-id>\n\
  cairn lesson <store> <content>\n\
  cairn correct <store> <memory-or-event-id> <content>\n\
  cairn export <store> <output>\n\
  cairn import <store> <input>\n\
  cairn account-bootstrap <store> <email> <password>\n\
  cairn project-register <store> <account-token> <repository> <name> [default-branch]\n\
  cairn project-list <store> <account-token>\n\
  cairn promote <store> <account-token> <project-id> <memory-id> <reason>\n\
  cairn project-memories <store> <account-token> <project-id>\n\
  cairn project-freshness <store> <account-token> <project-id>\n\
  cairn account-register <store> <email> <password>\n\
  cairn project-invite <store> <owner-token> <project-id> <email>\n\
  cairn project-share <store> <owner-token> <project-id> <memory-id> <email>\n\
  cairn project-shared <store> <member-token> <project-id>\n\
  cairn project-revoke <store> <owner-token> <project-id> <memory-id> <email>"
    );
    process::exit(2);
}

fn required(args: &[String], index: usize, name: &str) -> String {
    args.get(index).cloned().unwrap_or_else(|| {
        eprintln!("missing {name}");
        usage();
    })
}

fn optional(args: &[String], index: usize) -> Option<String> {
    args.get(index).filter(|value| !value.is_empty()).cloned()
}

fn context(args: &[String], start: usize) -> WorkContext {
    WorkContext {
        repository: optional(args, start),
        branch: optional(args, start + 1),
        worktree: optional(args, start + 2),
        commit: optional(args, start + 3),
        files: Vec::new(),
        ..WorkContext::default()
    }
}

fn recall_filter(args: &[String], start: usize) -> RecallFilter {
    if args.len() <= start {
        RecallFilter::default()
    } else {
        RecallFilter {
            session_id: None,
            context: Some(context(args, start)),
        }
    }
}

fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("result serializes")
    );
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or_else(|| {
        usage();
    });

    let result = match command {
        "init" => {
            let store = required(&args, 1, "store path");
            MemoryStore::open(store).map(|_| println!("Cairn store ready"))
        }
        "start" => {
            let store = required(&args, 1, "store path");
            let session_id = required(&args, 2, "session id");
            MemoryStore::open(&store)
                .and_then(|mut store| store.start_session(session_id, context(&args, 3)))
                .map(|session| print_json(&session))
        }
        "capture" => {
            let store = required(&args, 1, "store path");
            let session_id = required(&args, 2, "session id");
            let event_id = required(&args, 3, "event id");
            let content = required(&args, 4, "content");
            let role = optional(&args, 5).unwrap_or_else(|| "agent".to_owned());
            let source = optional(&args, 6).unwrap_or_else(|| "cli".to_owned());
            MemoryStore::open(&store)
                .and_then(|mut store| {
                    store.capture_observation(ObservationInput {
                        event_id,
                        session_id,
                        content,
                        role,
                        source,
                        importance: None,
                        lifecycle: LifecycleInput::default(),
                        context: WorkContext::default(),
                    })
                })
                .map(|capture| print_json(&capture))
        }
        "end" => {
            let store = required(&args, 1, "store path");
            let session_id = required(&args, 2, "session id");
            MemoryStore::open(&store)
                .and_then(|mut store| store.end_session(&session_id))
                .map(|memory| print_json(&memory))
        }
        "search" => {
            let store = required(&args, 1, "store path");
            let query = required(&args, 2, "query");
            let limit = optional(&args, 3)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(5);
            MemoryStore::open(&store)
                .and_then(|store| store.search_with_filter(&query, limit, &recall_filter(&args, 4)))
                .map(|results| print_json(&results))
        }
        "context" => {
            let store = required(&args, 1, "store path");
            let query = required(&args, 2, "query");
            let limit = optional(&args, 3)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(5);
            MemoryStore::open(&store)
                .and_then(|store| {
                    store.context_with_filter(&query, limit, &recall_filter(&args, 4))
                })
                .map(|pack| print_json(&pack))
        }
        "why-changed" => {
            let store = required(&args, 1, "store path");
            let query = required(&args, 2, "query");
            let limit = optional(&args, 3)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(5);
            MemoryStore::open(&store)
                .and_then(|store| {
                    store.why_changed_context(&query, limit, &recall_filter(&args, 4))
                })
                .map(|pack| print_json(&pack))
        }
        "resume" | "unresolved" => {
            let store = required(&args, 1, "store path");
            let limit = optional(&args, 2)
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(5);
            let filter = recall_filter(&args, 3);
            MemoryStore::open(&store)
                .and_then(|store| {
                    if command == "resume" {
                        store.resume_context(limit, &filter)
                    } else {
                        store.unresolved_context(limit, &filter)
                    }
                })
                .map(|pack| print_json(&pack))
        }
        "forget" => {
            let store = required(&args, 1, "store path");
            let id = required(&args, 2, "memory or event id");
            MemoryStore::open(&store).and_then(|mut store| store.forget(&id))
        }
        "lesson" => {
            let store = required(&args, 1, "store path");
            let content = required(&args, 2, "lesson content");
            MemoryStore::open(&store)
                .and_then(|mut store| {
                    store.remember_lesson(
                        &content,
                        Vec::new(),
                        WorkContext::default(),
                        LifecycleInput::default(),
                    )
                })
                .map(|memory| print_json(&memory))
        }
        "correct" => {
            let store = required(&args, 1, "store path");
            let target_id = required(&args, 2, "memory or event id");
            let content = required(&args, 3, "corrected content");
            MemoryStore::open(&store)
                .and_then(|mut store| {
                    store.correct(
                        &target_id,
                        &content,
                        Vec::new(),
                        WorkContext::default(),
                        LifecycleInput::default(),
                    )
                })
                .map(|memory| print_json(&memory))
        }
        "export" => {
            let store = required(&args, 1, "store path");
            let output = required(&args, 2, "export output path");
            MemoryStore::open(&store)
                .and_then(|store| store.export_json())
                .and_then(|json| fs::write(&output, json).map_err(Into::into))
        }
        "import" => {
            let store = required(&args, 1, "store path");
            let input = required(&args, 2, "export input path");
            fs::read_to_string(&input)
                .map_err(Into::into)
                .and_then(|raw| MemoryStore::open(&store)?.import_json(&raw))
                .map(|result| print_json(&result))
        }
        "account-bootstrap" => {
            let store = required(&args, 1, "store path");
            let email = required(&args, 2, "email");
            let password = required(&args, 3, "password");
            MemoryStore::open(&store)
                .and_then(|mut store| store.bootstrap_account(&email, &password))
                .map(|session| print_json(&session))
        }
        "project-register" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "account token");
            let repository = required(&args, 3, "repository");
            let name = required(&args, 4, "name");
            let default_branch = optional(&args, 5);
            MemoryStore::open(&store)
                .and_then(|mut store| {
                    store.register_project(&account_token, &repository, &name, default_branch)
                })
                .map(|project| print_json(&project))
        }
        "project-list" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "account token");
            MemoryStore::open(&store)
                .and_then(|store| store.projects(&account_token))
                .map(|projects| print_json(&projects))
        }
        "promote" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "account token");
            let project_id = required(&args, 3, "project id");
            let memory_id = required(&args, 4, "memory id");
            let reason = required(&args, 5, "reason");
            MemoryStore::open(&store)
                .and_then(|mut store| {
                    store.promote_memory(&account_token, &project_id, &memory_id, &reason)
                })
                .map(|memory| print_json(&memory))
        }
        "project-memories" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "account token");
            let project_id = required(&args, 3, "project id");
            MemoryStore::open(&store)
                .and_then(|store| store.project_memories(&account_token, &project_id))
                .map(|memories| print_json(&memories))
        }
        "project-freshness" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "account token");
            let project_id = required(&args, 3, "project id");
            MemoryStore::open(&store)
                .and_then(|store| store.freshness_history(&account_token, &project_id))
                .map(|events| print_json(&events))
        }
        "account-register" => {
            let store = required(&args, 1, "store path");
            let email = required(&args, 2, "email");
            let password = required(&args, 3, "password");
            MemoryStore::open(&store)
                .and_then(|mut store| store.register_account(&email, &password))
                .map(|session| print_json(&session))
        }
        "project-invite" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "owner token");
            let project_id = required(&args, 3, "project id");
            let email = required(&args, 4, "email");
            MemoryStore::open(&store)
                .and_then(|mut store| {
                    store.invite_project_member(&account_token, &project_id, &email)
                })
                .map(|membership| print_json(&membership))
        }
        "project-share" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "owner token");
            let project_id = required(&args, 3, "project id");
            let memory_id = required(&args, 4, "memory id");
            let email = required(&args, 5, "email");
            MemoryStore::open(&store)
                .and_then(|mut store| {
                    store.share_project_memory(&account_token, &project_id, &memory_id, &email)
                })
                .map(|share| print_json(&share))
        }
        "project-shared" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "member token");
            let project_id = required(&args, 3, "project id");
            MemoryStore::open(&store)
                .and_then(|store| store.shared_project_memories(&account_token, &project_id))
                .map(|memories| print_json(&memories))
        }
        "project-revoke" => {
            let store = required(&args, 1, "store path");
            let account_token = required(&args, 2, "owner token");
            let project_id = required(&args, 3, "project id");
            let memory_id = required(&args, 4, "memory id");
            let email = required(&args, 5, "email");
            MemoryStore::open(&store)
                .and_then(|mut store| {
                    store.revoke_project_share(&account_token, &project_id, &memory_id, &email)
                })
                .map(|share| print_json(&share))
        }
        _ => usage(),
    };

    if let Err(error) = result {
        eprintln!("cairn: {error}");
        process::exit(1);
    }
}
