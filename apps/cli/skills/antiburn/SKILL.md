---
name: antiburn
description: At the end of each turn, report new Antiburn findings that matter to the current work.
---

# Antiburn findings

At the end of each turn, run this command exactly once with the active session ID supplied by the harness. Pass the ID as one argument. If the harness does not supply it, skip the call. Do not ask the user or read a harness-specific environment variable.

```sh
antiburn checks '<session_id>'
```

Read the `checks` array in the JSON response. It contains only new or changed findings. If the array is empty or no finding matters to the current work, do not mention Antiburn. Do not inspect raw evidence, infer findings, report clean results, or print the JSON.

After your main answer, add a blank line and this standalone footer only when a finding is relevant to report:

```markdown
🔥 **antiburn**: <brief finding summary> · session=<session_id>
```

- Add at most one footer. Do not repeat a finding unless the CLI returns it again.
- Do not describe findings as current or verified or take remediation action based on them.
- The CLI resolves agent and environment from the local database. Do not guess either value.
- A failed call is not a clean result. Mention the failure only if the user asks about it.
