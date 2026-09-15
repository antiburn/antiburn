# Burn Checks summary without an anchored preview

## Corrected scope

Keep the normal menu-bar popover, tray controls, provider usage previews, and
onboarding behaviour. Hovering or focusing the Burn Checks summary must not
open the additional anchored checks window. Clicking the summary still opens
Burn Checks in the main window.

Preserve the native tray behaviour from PR #493. Keith reviewed the corrected
build and explicitly requested publishing a PR and assigning it to Zack.

| Step | Status |
| --- | --- |
| Restore the normal menu-bar behaviour | Complete |
| Remove only the checks hover/focus preview trigger | Complete |
| Verify provider previews and checks click navigation | 73 tests, lint, types, and formatting passed |
| Build and launch for Keith to test | Complete; awaiting Keith’s review |
| Add a clearer checks-card hover and focus treatment | Complete; 73 tests, lint, types, design drift, and build passed; launched for review |
| PR | Publishing authorized; assign to Zack; results tracked in the PR |
