---
id: CM-STD-002
title: "Cairn Rust Idioms and Review Rules"
status: accepted
---

# Cairn Rust Idioms and Review Rules

This is a repository instruction for every agent and contributor changing Rust
code in Cairn Memory. Read it before inspecting or editing `*.rs`, Cargo
manifests, migrations used by Rust code, or Rust-facing public contracts.

The goal is not to make every line look clever. The goal is code that uses
Rust's type system, ownership model, standard library conventions, and tooling
to make correctness visible, APIs predictable, failures explicit, and
maintenance inexpensive.

## 1. Working sequence

1. Read `AGENTS.md`, `README.md`, and `docs/README.md` when the
   change affects product behavior or architecture.
2. Identify the public boundary, callers, serialized data, migrations, and
   tests before changing an API or type.
3. Write the smallest complete change. Do not refactor unrelated code merely
   because it can be made prettier.
4. Run `cargo fmt --all` before review and `cargo fmt --all -- --check` before
   reporting success.
5. Run `cargo check --workspace`, `cargo clippy --workspace --all-targets
   --all-features -- -D warnings`, and the narrowest relevant tests.
6. Run the repository smoke checks when a transport, persistence, migration,
   lifecycle, synchronization, or public contract is affected.
7. Explain any deliberate deviation from these rules in the code review or
   change notes. Prefer a local `#[expect(..., reason = "...")]` over a broad
   lint suppression when a lint is intentionally not applicable.

## 2. Rust's design vocabulary

Prefer Rust concepts over patterns imported from object-oriented languages:

- Use free functions for operations that do not act on a receiver.
- Use inherent associated functions mainly for constructors such as `new`,
  `from_*`, `try_from_*`, or builders. Use methods when behavior acts on
  `self`.
- Use enums to represent a closed set of states and transitions. Do not encode
  meaningful states with unrelated booleans or unvalidated strings when the
  state is internal and closed.
- Use structs for named data that travels together. Keep public structs small,
  coherent, and constructible without exposing invariants that callers cannot
  uphold.
- Use traits for behavior shared by types or for a boundary that genuinely
  needs substitution. Do not add a trait only to hide one implementation or
  to imitate interfaces from another language.
- Prefer composition, modules, and functions over `Manager`, `Service`, or
  `Factory` types whose names conceal what they actually do.
- Keep state transitions explicit. If a transition has preconditions, expose a
  method whose name describes the transition and return a meaningful error when
  the preconditions fail.

## 3. Ownership, borrowing, and allocation

- Borrow immutable inputs as `&T`, `&str`, and slices. Accept `&[T]` instead of
  `&Vec<T>`, `&str` instead of `&String`, and `Option<&T>` instead of
  `&Option<T>` when ownership is not required.
- Take owned values when the function stores them, transfers them, or needs to
  return them without cloning. Make that ownership transfer clear at the call
  site.
- Prefer borrowing over cloning. A clone is justified when an owned value must
  outlive a borrow, when a snapshot is intentionally required, or when the
  clone is cheap and removes genuinely complex lifetime coupling.
- Do not clone merely to silence the borrow checker. First consider narrowing
  a borrow's scope, extracting a value before taking a mutable borrow, using
  `Option::take`, returning owned data, or splitting a function.
- Avoid `Arc`, `Rc`, `Mutex`, and other shared ownership or interior-mutability
  tools unless shared ownership or mutation is part of the actual design.
  `Arc` and locks are appropriate at process or task boundaries; they are not a
  default replacement for clear ownership.
- Avoid unnecessary intermediate `String`, `Vec`, and JSON allocations. Use
  slices, iterators, `Cow`, `write!`, and borrowed query inputs where the API
  permits them.
- Use `String` for owned text and `&str` for borrowed text. Use path types for
  filesystem paths rather than treating paths as ordinary strings internally.
- Make collection capacity decisions only when a real bound or measurement
  supports them. Do not optimize speculative allocations at the cost of
  clarity.

## 4. `Option`, `Result`, and invariants

- Use `Option<T>` when absence is an expected result and no additional failure
  explanation is useful.
- Use `Result<T, E>` when the caller needs to distinguish or report failure,
  especially for I/O, database, parsing, authentication, validation, and
  public API operations.
- Use `?` to propagate errors. Add context at the boundary where it becomes
  useful, without destroying the original cause.
- Define error types around recoverable categories. Keep validation errors
  distinguishable from infrastructure failures and unauthorized access.
- Do not use `unwrap` or `expect` for user input, network data, database rows,
  filesystem state, or any condition that production input can violate.
- A panic is acceptable only for an actual programmer invariant that has been
  established immediately before the operation, or in tests where the panic
  is the assertion mechanism. The invariant must be obvious and local.
- Replace `expect` used as a disguised control-flow branch with a checked
  lookup and a domain error. If an invariant really cannot fail, prefer a
  structure that makes it impossible or document the exact proof next to the
  assertion.
- Never turn a recoverable error into an empty success, silently discard a
  `Result`, or use `unwrap_or_default` where absence changes behavior.
- Keep error messages actionable, stable enough for tests and operators, and
  free of secrets, bearer tokens, passwords, private content, or unnecessary
  personal data.

## 5. API and type conventions

- Follow standard Rust naming: `snake_case` for functions and fields,
  `UpperCamelCase` for types and traits, and `SCREAMING_SNAKE_CASE` for
  constants.
- Use short names that carry meaning. Avoid redundant crate or type prefixes.
- Name conversions consistently: `as_*` for cheap borrowed views, `to_*` for
  an owned conversion, and `into_*` for consuming conversion.
- Name getters after the value, such as `status()` or `project_id()`, unless a
  `get_*` name avoids ambiguity or follows an established collection API.
- Implement common traits when they describe the type: `Debug` for public
  types, `Clone` and equality traits when semantically valid, `Default` when a
  meaningful default exists, and `Display` for user-readable values and
  errors. Do not derive traits that expose secrets or imply false semantics.
- Use `#[derive(Default)]` with a `#[default]` enum variant when the default is
  a simple variant choice. Keep manual implementations for computed or
  invariant-bearing defaults.
- Keep public data types serializable only when serialization is part of their
  contract. Use `#[serde(default)]` for backward-compatible optional fields,
  and update migrations and import/export tests when persisted shape changes.
- Prefer explicit enums for internal lifecycle and status values. Preserve
  strings only at compatibility boundaries or when users must store open-ended
  values; validate them at the boundary.
- Make constructors validate required invariants. Use `new` for infallible
  construction and `try_new`/`from_*` returning `Result` when input can fail.

## 6. Control flow and pattern matching

- Match on enums and meaningful states exhaustively. Use a wildcard only when
  future variants truly have identical behavior and the compatibility policy
  requires it.
- Prefer `if let` or `let else` for one meaningful pattern and `match` when
  multiple outcomes need distinct behavior.
- Use let-chain syntax to combine related guards without deeply nested `if`
  blocks.
- Prefer iterator adapters when they express the whole transformation clearly:
  `filter`, `map`, `filter_map`, `find`, `any`, `all`, `partition`, and
  `collect`. Use a `for` loop when mutation, early return, side effects, or
  multi-step control flow is clearer.
- Do not force a long iterator chain into a dense expression. Readability and
  correct error handling are more important than appearing functional.
- Prefer `matches!`, `is_some_and`, `is_none_or`, `then_some`, and direct
  standard-library predicates when they make intent clearer.
- Avoid needless boolean comparisons such as `== true`; use the boolean value
  directly.
- Use checked arithmetic or explicit bounds for cursors, lengths, indexes, and
  externally supplied numeric values.

## 7. Async, locking, and database code

- Keep async functions asynchronous at the I/O boundary. Do not block an async
  executor with synchronous filesystem, process, or CPU-heavy work without a
  deliberate boundary.
- Hold locks for the shortest scope that preserves the invariant. Do not await
  while holding a lock unless the lock is intentionally part of the async
  transaction and the deadlock implications are understood.
- Extract or clone only the owned snapshot needed before releasing a lock.
- Use transactions for multi-step state transitions that must be atomic. Lock
  the row or logical record before checking and changing a state when concurrent
  requests could otherwise both succeed.
- Keep SQL ownership predicates scoped by the authenticated person on every
  read, duplicate check, update, and delete. A public identifier is never a
  sufficient ownership proof.
- Prefer parameterized SQL. Never concatenate user data into SQL text. Dynamic
  SQL may build only trusted structural fragments such as fixed predicates.
- Map database rows into domain types in one focused conversion function. Keep
  parsing and validation errors visible instead of manufacturing defaults.
- Make migration order explicit and test a fresh schema. If a persisted field
  changes, preserve older data or provide a migration and a compatibility test.

## 8. Serialization, privacy, and public boundaries

- Validate and normalize at the boundary, then keep internal code working with
  validated values.
- Redact secrets before persistence, logs, error messages, audit metadata, and
  test output. Do not rely on callers to redact correctly.
- Preserve provenance when deriving or promoting records. Do not mutate the
  source merely to create a broader-scope representation.
- Treat JSON and database values as untrusted input. Bound sizes, collections,
  limits, and recursion-like work before processing them.
- Use explicit serde aliases only for compatibility that the contract requires.
  Avoid accepting many spellings without a reason.
- Keep HTTP handlers thin: extract, authenticate, call a domain/storage method,
  and map the result. Put validation, ownership, and transitions below the
  transport layer so callers behave consistently.

## 9. Documentation and comments

- Comment why a non-obvious invariant, limit, timeout, migration, or policy
  exists. Do not narrate obvious syntax.
- Use rustdoc for public types and methods whose contract is not obvious from
  their signature. Include examples when they clarify the intended API.
- Keep comments current with the code. A stale comment is worse than no
  comment because it teaches the next contributor the wrong invariant.
- Name constants for policy values and document their operational reason. Do
  not scatter unexplained numeric limits through control flow.
- Put product and trust-boundary decisions in the Decisions section of `docs/architecture.md` or `docs/release.md`, not only in
  source comments.

## 10. Static checks and review gates

The minimum Rust quality gate for this workspace is:

```text
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Use additional checks when relevant:

- Run the local smoke scripts after transport, persistence, migration, or
  lifecycle changes.
- Run the PostgreSQL integration and smoke flow after SQL or storage changes.
- Use dependency auditing and unused-dependency checks when dependency changes
  are made.
- Use sanitizers or Miri only when unsafe code or low-level concurrency makes
  them relevant. This workspace should prefer safe Rust and currently has no
  reason to add unsafe code.
- Treat every Clippy warning as a design review prompt. Fix it when it points
  to clearer or safer code; otherwise use a narrowly scoped, reasoned
  `#[expect]` and record why the exception is correct.

## 11. Cairn-specific priorities

When general idioms conflict with a Cairn contract, preserve the contract and
document the deliberate choice. In particular:

- person ownership and scope checks outrank convenience;
- fail-closed behavior outranks permissive recovery;
- source evidence and provenance outrank deduplication convenience;
- compatibility aliases and serialized fields are public behavior;
- migration safety outranks a locally simpler schema;
- bounded work and redaction outrank cleverness or micro-optimization;
- PostgreSQL remains the authoritative store for owner-facing behavior.

## Review checklist

- [ ] The agent read this file before changing Rust.
- [ ] Public callers, serialized forms, migrations, and tests were inspected.
- [ ] Inputs are borrowed or owned intentionally, with no unexplained clone.
- [ ] `Option`, `Result`, and panic behavior match the failure contract.
- [ ] State transitions validate ownership and current state atomically.
- [ ] Secrets and private data cannot enter errors, logs, or audit metadata.
- [ ] Names, conversions, traits, and constructors follow Rust conventions.
- [ ] The change is formatted and Clippy-clean, or every exception is reasoned.
- [ ] Relevant behavior tests and smoke checks passed.
