# cairn-server

## What
The REST transport and the background worker around `cairn-store`. It authenticates web sessions and
device tokens, maps store errors to `{error, code}` responses, and runs the embed, distill and
consolidate jobs with the local MiniLM model. Handlers are in `src/routes/`.

## Entry points
- `main.rs`: `--database-url` (`CAIRN_DATABASE_URL`), `--schema`, `--bind`, `--embedding-model-dir`.
- `router(AppState)`; `worker::spawn` (the process) and `worker::tick` (tests).
- `auth.rs` (`agent`, `device`, `web`, `admin` extractors, rate limiter), `error.rs` (`ApiError`).
- `assets/minilm/` pins the embedding model (digests only; weights come from `scripts/download_embedding_model.py`).

## Depends on
`cairn-store`, `cairn-core`, `axum`, `fastembed`.
