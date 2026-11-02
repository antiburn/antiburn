---
name: antiburn
description: At the end of each turn, report new Antiburn findings that matter to the current work.
---

# Antiburn findings

At the end of each turn, call `antiburn checks <session_id>` exactly once. Replace `<session_id>` with the active session ID supplied by the harness. Do not ask the user for it or read a harness-specific environment variable. If the harness cannot provide the ID, skip the call.

The CLI returns only new or changed problematic checks. Read the `checks` array. If it is empty, do not mention Antiburn. If it contains findings relevant to the current work, summarize them briefly. Do not inspect raw evidence, infer findings, report clean results, or print the JSON.

After your main answer, add a blank line and this standalone footer only when a finding is relevant to report:

```markdown
🔥 **antiburn**: <brief finding summary> · session=<session_id>
```

Do not take remediation action based on a check result.

---

The CLI resolves the agent and Antiburn environment from its local database. The skill does not guess these values.

The CLI does not prove that a returned finding is current. Do not describe it as current or verified.

If the call fails, do not treat the failure as a clean result. Do not mention Antiburn unless the user asks about the failed check.

Do not report the same finding again unless the CLI returns it again.

Do not add multiple Antiburn footers.
