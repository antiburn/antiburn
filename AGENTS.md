# Agent instructions

These instructions apply to the entire repository.

## React effects

Avoid `useEffect` by default. Prefer deriving values during render, handling work in the event that caused it, or moving synchronization to the external-system boundary.

Only add a `useEffect` when it is strictly necessary to synchronize React with an external system and no simpler design provides the required behavior. Before adding one, explain why it is necessary and get explicit agreement from the developer. Existing effects are not precedent for adding more, and reviews should call out effects that can be removed.

## Rust dead and deprecated code

Do not add or retain Rust lint suppressions for dead or deprecated code. This includes item-level forms such as `#[allow(dead_code)]` and `#[allow(deprecated)]`, crate- or module-level forms such as `#![allow(dead_code)]` and `#![allow(deprecated)]`, and the corresponding `#[expect(...)]` forms.

Delete dead code and migrate away from deprecated APIs instead. A suppression is permitted only after explaining the concrete need and receiving explicit agreement from the developer. Existing suppressions are not precedent for adding or retaining more, and reviews should call them out.
