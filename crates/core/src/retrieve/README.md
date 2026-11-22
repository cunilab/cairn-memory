# retrieve

## What
The ranking and shaping of recall results over memories the store already loaded and filtered by
visibility: request filters, reciprocal-rank fusion of the search channels, the relevance gate,
capped score adjustments, result packing (diversity cap, token budget, limit), the ranked session
brief and the memory graph.

## Entry points
- `filter::{Filter, matches}`; `hybrid::{fuse, RRF_K}`; `gate::{keep, GateInput}`.
- The store applies the request's project and kind inside every channel (`Narrow`) and caps the lexical legs; only a name in the query text can pass the gate through the entity channel.
- `rank::{adjust, status_after_votes}` (adjustments capped at ±30 %); `pack::{pack, Cards, Hit}`.
- `brief::session_brief` (at most 1,200 bytes); `graph::{build, node_cap, Graph}`.

## Depends on
`memory`, `capture`, `process`.
