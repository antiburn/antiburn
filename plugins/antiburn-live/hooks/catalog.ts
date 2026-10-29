// The hand-maintained live check catalog (plan decision 16).
// crates/antiburn-local/tests/live_catalog_drift.rs reads the JSON between the
// two markers and compares it with ReportCatalogs::default(). Keep it valid JSON.
export const CATALOG = /* catalog:begin */
{
  "schema": 1,
  "revision": 2,
  "date": "2026-10-09",
  "reportCatalogRevision": 8,
  "modelRegistryRevision": 2,
  "policy": {
    "depthCapTokens": 400000,
    "claude": {
      "effortAboveCap": ["max", "ultra", "xhigh"],
      "premiumSubstrings": ["fable", "mythos", "opus"]
    },
    "modelReplacements": {
      "claude-opus-4-5": "claude-opus-5",
      "claude-opus-4-6": "claude-opus-5",
      "claude-opus-4-6-thinking": "claude-opus-5",
      "claude-opus-4-7": "claude-opus-5",
      "claude-opus-4-8": "claude-opus-5",
      "claude-opus-4-8-thinking-high": "claude-opus-5",
      "claude-opus-4.6": "claude-opus-5",
      "claude-opus-4.7": "claude-opus-5",
      "claude-opus-4.8": "claude-opus-5",
      "claude-sonnet-4-5": "claude-sonnet-5",
      "claude-sonnet-4-6": "claude-sonnet-5",
      "claude-sonnet-4.6": "claude-sonnet-5",
      "gpt-5.4": "gpt-5.6-terra",
      "gpt-5.4-mini": "gpt-5.6-luna",
      "gpt-5.5": "gpt-5.6-sol",
      "gpt-5.5-fast": "gpt-5.6-sol"
    }
  },
  "live": {
    "retryMinRepeats": 3,
    "retrySteps": [3, 5, 10, 20],
    "contextPercent": 90,
    "contextReadings": 2,
    "effortMinRequests": 5,
    "effortMinShare": 0.5,
    "graceTurns": 3,
    "watchTurns": 3,
    "readMinCalls": 20,
    "readMinPaths": 10,
    "readMinShare": 0.5,
    "skillMinSteps": 10,
    "skillMinKeywords": 3,
    "planMinFiles": 3,
    "planCleanTurns": 3,
    "workflowMinCommands": 3,
    "workflowMinRepeats": 3
  },
  "checks": [
    {
      "id": "X1",
      "order": 1,
      "label": "Retry loop",
      "area": "tools",
      "basis": "turn",
      "sentence": "The same tool call failed several times in one turn. Each retry sends the whole context again.",
      "action": { "kind": "submit", "label": "Ask Claude to fix" },
      "prompt": "Stop retrying the call that keeps failing. Read its last error, say in one line what causes it, and change the approach before you run it again."
    },
    {
      "id": "E1",
      "order": 5,
      "label": "Reading spiral",
      "area": "tools",
      "basis": "turn",
      "sentence": "Claude read many files in one turn and changed none. A long read with no edit often means the plan is not settled yet.",
      "action": { "kind": "submit", "label": "Ask Claude to focus" },
      "prompt": "Pause the search. In five lines, state what you found, your current hypothesis and the files you will change. Then make the change, or ask me one question if the task is unclear."
    },
    {
      "id": "D",
      "order": 2,
      "label": "Context heavy",
      "area": "context",
      "basis": "turn",
      "sentence": "Each request now carries a very large context. Every step pays for all of it again.",
      "action": { "kind": "compact", "label": "Compact now" },
      "prompt": "Keep the current task, the decisions made, the open questions and the files in progress. Drop tool output that the task no longer needs.",
      "readsPrompt": " Keep findings and file paths from the search; drop the raw file contents."
    },
    {
      "id": "O",
      "order": 3,
      "label": "Obsolete model",
      "area": "model",
      "basis": "turn",
      "sentence": "This session runs on a model that has a newer replacement. A switch is cheap before the context grows.",
      "action": { "kind": "fill", "label": "Switch model" },
      "prompt": "/model {replacement}"
    },
    {
      "id": "T",
      "order": 4,
      "label": "Overthinking",
      "area": "model",
      "basis": "accumulated",
      "sentence": "Most requests in this session use an effort level above the recommended cap. Routine steps rarely need it.",
      "action": { "kind": "fill", "label": "Lower effort" },
      "prompt": "/effort high"
    },
    {
      "id": "K1",
      "order": 6,
      "label": "Skill not used",
      "area": "skills",
      "basis": "turn",
      "sentence": "This request matched an installed skill, but Claude did the work by hand over many steps. The skill may do it in fewer next time.",
      "action": { "kind": "advise", "label": "Got it" },
      "prompt": "/{skill}"
    },
    {
      "id": "C1",
      "order": 7,
      "label": "Outside the plan",
      "area": "scope",
      "basis": "accumulated",
      "sentence": "Since you approved the plan, Claude edited files that neither the plan nor your messages name. Extra changes cost tokens now and review later.",
      "action": { "kind": "fill", "label": "Ask about extras" },
      "prompt": "List each change you made outside the approved plan, one line each with the reason. Keep only the ones I approve, and ask before you add work beyond the plan."
    },
    {
      "id": "K2",
      "order": 8,
      "label": "Repeated workflow",
      "area": "skills",
      "basis": "accumulated",
      "sentence": "The same set of commands ran in several turns of this session. A skill can hold the steps, so each run needs fewer instructions.",
      "action": { "kind": "fill", "label": "Draft a skill" },
      "prompt": "Write a skill for the steps you repeated this session: {commands}. Put it in .claude/skills/<name>/SKILL.md with a one-line description of when to use it. Show it to me before you save it."
    }
  ],
  "acrossSessions": ["M", "B", "K", "I", "F"]
} /* catalog:end */ as const
