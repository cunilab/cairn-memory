# project

## What
Project identity and visibility: the project record, the text rules for names, and the predicate that
says whether a person may see a memory (owner, project member, or global).

## Entry points
- `Project`, `validate_project_text`, `default_project_name`.
- `access::{Viewer, is_visible}` (the same rule the store applies in SQL).

## Depends on
`memory`.
