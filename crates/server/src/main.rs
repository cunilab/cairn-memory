use cairn_server::AppState;
use cairn_server::app;
use cairn_server::{LocalEmbeddings, postgres_app_with_embeddings};
use cairn_storage::PostgresStore;
use std::env;
use std::future::Future;
use std::net::SocketAddr;
use std::process;
use std::time::Duration;

/// How often sessions idle for over 30 minutes are closed without waiting for new activity.
const IDLE_SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);

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
    let router = if let Some(database_url) =
        option_value(&args, "--database-url").or_else(|| env::var("CAIRN_DATABASE_URL").ok())
    {
        let store = PostgresStore::connect(&database_url)
            .await
            .unwrap_or_else(|error| {
                eprintln!("failed to connect to Cairn PostgreSQL storage: {error}");
                process::exit(1);
            });
        let sweeper = store.clone();
        spawn_idle_sweep(move || {
            let store = sweeper.clone();
            async move { store.sweep_idle_sessions().await }
        });
        postgres_app_with_embeddings(store, embeddings)
    } else {
        let store_path = option_value(&args, "--store").unwrap_or_else(|| {
            eprintln!("missing --store <path> or --database-url <url>");
            usage_and_exit();
        });
        let mut state = AppState::open(store_path).unwrap_or_else(|error| {
            eprintln!("failed to open Cairn store: {error}");
            process::exit(1);
        });
        if let Some(model) = embeddings {
            state = state.with_embeddings(model);
        }
        let sweeper = state.clone();
        spawn_idle_sweep(move || {
            let state = sweeper.clone();
            async move { state.sweep_idle_sessions().await }
        });
        app(state)
    };
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .unwrap_or_else(|error| {
            eprintln!("failed to bind {address}: {error}");
            process::exit(1);
        });

    println!("Cairn REST server listening on http://{address}");
    if let Err(error) = axum::serve(listener, router).await {
        eprintln!("Cairn server stopped: {error}");
        process::exit(1);
    }
}

fn spawn_idle_sweep<F, Fut>(sweep: F)
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(IDLE_SWEEP_INTERVAL);
        tick.tick().await; // the first tick is immediate; a fresh start has nothing idle to close
        loop {
            tick.tick().await;
            sweep().await;
        }
    });
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn usage_and_exit() -> ! {
    eprintln!(
        "Usage: cairn-server --store <path> [--bind 127.0.0.1:3111] [--embedding-model-dir <path>]"
    );
    eprintln!(
        "   or: cairn-server --database-url <postgres-url> [--bind 127.0.0.1:3111] [--embedding-model-dir <path>]"
    );
    process::exit(2);
}
