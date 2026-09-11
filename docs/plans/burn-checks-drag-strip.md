# Burn Checks window dragging

Add a macOS-only, fixed 40px drag strip above the Burn Checks scroll area.
Use the existing main-window titlebar height and Tauri drag-region behavior.
Preserve sidebar dragging and keep report controls outside the drag region.
The review build was provided to Keith. Keith explicitly requested publishing
the PR, assigning Zack, and replying to Marty’s Slack thread.

| Step | Status |
| --- | --- |
| Add drag strip for loaded, loading, and unavailable states | Complete |
| Verify macOS-only chrome and run checks | 55 tests, lint, types, formatting, and design drift passed |
| Build and launch for review | Complete; awaiting manual drag verification |

| Publish and assign PR | Authorized; results tracked in the PR |
