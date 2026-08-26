# Local session titles (Mac scope)

Codex sessions show up in antiburn named by their raw first message — `[Image #1] in this pane, it should be possible to click…` — because Codex almost never generates a title of its own. This plan adds a local title pipeline on macOS: clean up the fallback everywhere, and generate real titles with Apple's on-device Foundation Models where available. No API tokens are ever spent naming sessions.

Approved 2026-08-24 after review. Windows (Phi Silica) and Ollama backends are follow-ups, out of this branch's scope.

## Status

| Step | Status |
| --- | --- |
| 1. Heuristic fallback cleanup | Done (marker strip + first sentence + word-boundary truncation in `clean_first_message_title`, applied in `select_title_pair`) |
| 2. `TitleSummarizer` trait + storage | Done (`TitleSummarizer` trait + `sanitize_generated_title` in the engine; `localSummary` provenance, guarded store writes, candidate collection in the scan, `local_summary_pass` wired after upsert — `platform_summarizer()` returns `None` until step 3. Follow-up fix 2026-08-25: candidate collection skips Codex-injected user turns — `# AGENTS.md instructions …` and `<environment_context>`-style elements — found when running the pipeline against Keith's real rollouts; without the filter the model titled the boilerplate) |
| 3. macOS backend (Apple Foundation Models sidecar) | Done (generic Swift sidecar `sidecar/run-foundation-model.swift` — a prompt-in/text-out Foundation Models runner, reusable beyond titles per PR review — compiled by build.rs and bundled via `tauri.macos.conf.json` externalBin; `SidecarSummarizer` runs it with a 20s timeout; `local_summary_titles` setting, default on, macOS-only toggle in General settings; generate-once per session, 15 titles per pass, newest first) |

## Background (investigated 2026-08-24)

- Antiburn's Codex title chain is already correct: user rename (`threads.name`) → Codex-generated title (`threads.title` / `session_index.jsonl`) → first user message ([codex.rs](../../crates/antiburn-local/src/discovery/agents/codex.rs)). The problem is upstream: on Keith's machine 322 of 329 Codex threads have `title` byte-identical to `first_user_message`. Only 7 ever got a generated name. The fallback IS the experience.
- Antiburn tracks provenance per title via `TitleSource` (`userRename` / `aiGenerated` / `explicit` / `firstMessage`) in [scanner.rs](../../crates/antiburn-local/src/discovery/scanner.rs). A generated title slots in as a new variant.
- Apple Foundation Models reports `available` on macOS 27. A Swift proof-of-concept titled real first messages in 300ms–1s each, on device, free. Quality with only the first message is mediocre; include repo name + first few turns for context.
- FoundationModels is Swift-only (no Rust ABI). Integration is a small generic Swift sidecar binary bundled via Tauri (stdin `{instructions, prompt}` JSON → stdout model response), spawned from Rust with a timeout; the title prompt is built in Rust. Reviewed and agreed in discuss.
- Open question: `~/.codex` has no rollouts or new thread rows since Aug 6, yet the HUD showed a Codex session 12h ago. Newer Codex builds may have moved local storage. Verify against a live session before relying on any Codex store (gate for step 3).

## Design

### Step 1 — heuristic cleanup (all platforms, always)

When resolution lands on `firstMessage`, clean before display:

- Strip attachment markers (`[Image #1]`, `[Pasted text #2]`, etc.) and leading command noise.
- Collapse whitespace/newlines; take the first sentence.
- Truncate at a word boundary (~60 chars) with an ellipsis.

Pure function in `antiburn-local`, unit-tested against real ugly samples. The `TitleSource` stays `firstMessage` — cleanup is presentation, not provenance.

### Step 2 — `TitleSummarizer` trait + storage

```rust
trait TitleSummarizer {
    async fn availability(&self) -> SummarizerAvailability; // Available / Unavailable(reason)
    async fn title(&self, input: &TitleInput) -> Option<String>;
}
```

- `TitleInput`: repo/dir name, first user message, and the first N assistant/tool turns (bounded, e.g. 2KB).
- Runs only when the resolved source is `firstMessage`. A later real Codex title, AI title, or user rename always wins.
- Result stored on the `session` row with a new `TitleSource::LocalSummary` (serialized `localSummary`), ranked between `firstMessage` and `aiGenerated`.
- Generated once per session and cached; regenerate only if the session had <2 turns when first titled and has since grown.
- Titles generate in a background queue after scan, never on the scan hot path. HUD shows the cleaned fallback until the summary lands.
- Behind a setting (`settings` table) defaulting on.

### Step 3 — Apple Foundation Models sidecar

- Small generic Swift helper binary (`run-foundation-model`) bundled with the app via Tauri's sidecar mechanism (stdin `{instructions, prompt}` JSON → stdout model response). Compiled only on macOS; the title prompt lives in Rust.
- Availability = `SystemLanguageModel.default.availability` (macOS 26+, Apple Intelligence enabled, supported hardware). Probe per run — the user can toggle Apple Intelligence off. Unavailable → step 1 cleanup only.
- Prompt: "You name coding-agent chat sessions… 3–6 words, imperative mood, no quotes, no trailing period." Include repo name and early turns.
- Guardrail: model output is untrusted — trim, cap length, strip newlines/quotes, reject empty or refusal-looking output and keep the fallback.

### Out of scope for this branch

- Windows Phi Silica backend (experimental App SDK, Developer Mode gate).
- Ollama backend.
- Shipping/downloading model weights.
- Using agent APIs (Claude, Codex) to title sessions — burning tokens to report burn is off-brand.
- Titling non-Codex agents. The pipeline is agent-agnostic, so widening later is cheap.

## Risks

- **Codex storage moved**: recent sessions aren't in any known local store. Verify the title chain against a live session before step 3.
- **Title quality**: a bad generated title is worse than an honest first-message fallback. Keep the guardrails strict and the `localSummary` provenance visible.
