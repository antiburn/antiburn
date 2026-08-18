# Agent instructions

These instructions apply to the entire repository.

## React effects

Avoid `useEffect` by default. Prefer deriving values during render, handling work in the event that caused it, or moving synchronization to the external-system boundary.

Only add a `useEffect` when it is strictly necessary to synchronize React with an external system and no simpler design provides the required behavior. Before adding one, explain why it is necessary and get explicit agreement from the developer. Existing effects are not precedent for adding more, and reviews should call out effects that can be removed.
