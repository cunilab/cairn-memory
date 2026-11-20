use cairn_server::{AppState, LocalEmbeddings, router, worker};
use cairn_store::Store;
use std::env;
use std::net::SocketAddr;
use std::process;

/// The production schema. `--schema` (or `CAIRN_SCHEMA`) picks another one in the same database,
/// which is how the smoke scripts share one disposable database.
const DEFAULT_SCHEMA: &str = "cairn";

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("cairn-server {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let bind = option_value(&args, "--bind").unwrap_or_else(|| "127.0.0.1:3111".to_owned());
    let address: SocketAddr = bind.parse().unwrap_or_else(|error| {
        eprintln!("invalid --bind value {bind}: {error}");
        process::exit(2);
    });
    let database_url = option_value(&args, "--database-url")
        .or_else(|| env::var("CAIRN_DATABASE_URL").ok())
        .unwrap_or_else(|| {
            eprintln!("missing --database-url <url> or CAIRN_DATABASE_URL");
            eprintln!(
                "Usage: cairn-server --database-url <postgres-url> [--bind 127.0.0.1:3111] [--schema cairn] [--embedding-model-dir <path>]"
            );
            process::exit(2);
        });
    let model_dir = option_value(&args, "--embedding-model-dir")
        .or_else(|| env::var("CAIRN_EMBEDDING_MODEL_DIR").ok());
    let embeddings =
        model_dir.and_then(
            |path| match LocalEmbeddings::load(std::path::Path::new(&path)) {
                Ok(model) => Some(model),
                Err(_) => {
                    eprintln!(
                        "local embedding model unavailable; using lexical/entity/link recall"
                    );
                    None
                }
            },
        );
    let schema = option_value(&args, "--schema")
        .or_else(|| env::var("CAIRN_SCHEMA").ok())
        .unwrap_or_else(|| DEFAULT_SCHEMA.to_owned());
    let store = Store::open(&database_url, &schema)
        .await
        .unwrap_or_else(|error| {
            eprintln!("failed to open Cairn PostgreSQL storage: {error}");
            process::exit(1);
        });
    let state = AppState::new(store, embeddings);
    worker::spawn(&state);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .unwrap_or_else(|error| {
            eprintln!("failed to bind {address}: {error}");
            process::exit(1);
        });
    println!("Cairn REST server listening on http://{address}");
    if let Err(error) = axum::serve(listener, router(state)).await {
        eprintln!("Cairn server stopped: {error}");
        process::exit(1);
    }
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}
