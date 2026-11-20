# retrieve

## What
The ranking and shaping of recall results over memories the store already loaded and filtered by
visibility: filters and the relevance floor, reciprocal-rank fusion of the search channels, usage
multipliers, result packing, the session brief and the memory graph.

## Entry points
- `filter::{Filter, matches, passes_relevance_floor}`; `hybrid::{fuse, kind_weight}`.
- `rank::{multiplier, status_after_wrong}`; `pack::{pack, Pack, Hit}`.
- `brief::session_brief` (at most 4,096 bytes); `graph::{build, node_cap, Graph}`.

## Depends on
`memory`, `capture`, `process`.
