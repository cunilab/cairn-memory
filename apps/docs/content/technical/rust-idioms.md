---
id: CM-STD-002
title: "Cairn Rust Idioms and Review Rules"
status: accepted
description: "Cairn’s Rust idioms and review checks for correctness, readability and privacy."
---

# Cairn Rust Idioms and Review Rules

Coding standards, type safety principles, memory efficiency, and review criteria for Rust contributions.

Every agent and contributor modifying Rust code in Cairn Memory must adhere to these standards.

## 1. Working sequence

1. Review `AGENTS.md`, `README.md`, and technical specifications before modifying behavior.
2. Identify public boundaries, callers, serialized types, and migrations before modifying APIs.
3. Keep changes minimal and focused. Avoid speculative or aesthetic refactoring.
4. Format code using `cargo fmt --all -- --check`.
5. Run workspace checks: `cargo check --workspace` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
6. Run smoke scripts (`smoke_mcp.sh`, `smoke_web.sh`) when touching persistence, migrations, or transport.

## 2. Rust design vocabulary

- **Domain modeling**: Model closed state spaces with `enum`. Avoid unvalidated strings or ambiguous boolean flags.
- **Structs**: Use structs for cohesive data. Keep fields private when enforcing invariants.
- **Traits**: Introduce traits only when genuine substitution or abstraction across crates is required.
- **Functions**: Prefer free functions and inherent methods over object-oriented patterns (`Manager`, `Service`, `Factory`).
- **Composition**: Keep data transformations explicit and deterministic.

## 3. Ownership, borrowing, and allocation

- Borrow rather than clone where data is only inspected (`&str`, `&[T]`).
- Prefer owned values when data is stored or transferred across asynchronous boundaries.
- Keep `cairn-core` completely free of asynchronous runtimes, PostgreSQL bindings, and network clients.

## 4. Error handling

- Domain errors must be strongly typed with `thiserror`.
- Mark errors clearly as client-correctable (`Validation`, `NotFound`, `Conflict`) or internal.
- Redact secrets before embedding strings in error messages.

## 5. Tests and review

- Unit tests belong in a sibling `tests.rs` module declared by `#[cfg(test)] mod tests;`.
- Integration and database tests belong under `crates/<crate>/tests/`.
- Never point tests at live user databases; use disposable schemas.
