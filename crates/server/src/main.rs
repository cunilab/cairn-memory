use cairn_server::AppState;
use cairn_server::app;
use cairn_server::mcp;
use cairn_server::postgres_app;
use cairn_storage::PostgresStore;
use std::env;
use std::net::SocketAddr;
use std::process;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("mcp") {
        run_mcp(&args[1..]);
        return;
    }
    let bind = option_value(&args, "--bind").unwrap_or_else(|| "127.0.0.1:3111".to_owned());
    let address: SocketAddr = bind.parse().unwrap_or_else(|error| {
        eprintln!("invalid --bind value {bind}: {error}");
        process::exit(2);
    });
    let router = if let Some(database_url) =
        option_value(&args, "--database-url").or_else(|| env::var("CAIRN_DATABASE_URL").ok())
    {
        let store = PostgresStore::connect(&database_url)
            .await
            .unwrap_or_else(|error| {
                eprintln!("failed to connect to Cairn PostgreSQL storage: {error}");
                process::exit(1);
            });
        postgres_app(store)
    } else {
        let store_path = option_value(&args, "--store").unwrap_or_else(|| {
            eprintln!("missing --store <path> or --database-url <url>");
            usage_and_exit();
        });
        let state = AppState::open(store_path).unwrap_or_else(|error| {
            eprintln!("failed to open Cairn store: {error}");
            process::exit(1);
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

fn option_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn usage_and_exit() -> ! {
    eprintln!("Usage: cairn-server --store <path> [--bind 127.0.0.1:3111]");
    eprintln!("   or: cairn-server --database-url <postgres-url> [--bind 127.0.0.1:3111]");
    eprintln!("   or: cairn-server mcp --store <path>");
    process::exit(2);
}

fn run_mcp(args: &[String]) {
    let store_path = option_value(args, "--store").unwrap_or_else(|| {
        eprintln!("missing --store <path>");
        usage_and_exit();
    });
    if let Err(error) = mcp::run_stdio(store_path) {
        eprintln!("Cairn MCP server stopped: {error}");
        process::exit(1);
    }
}
