# System One provider contracts

Use this reference to check whether a decision connection can return the typed
answers a Burn Check needs. These accepted wire contracts and local limits do not
prove model accuracy, source coverage, or support for every YAML feature. See the
[supported subset](authoring.md#supported-subset) for language support and
[decision connections](authoring.md#decision-connections) for setup.

Burn Checks use the experimental `v1-alpha` draft. They are not yet standardized,
and breaking changes have no backward compatibility promise. Beta and stable
scope are not set. Unsupported language or source features remain unavailable,
even when a provider supports a related wire feature.

## Question types and criteria

| Selected route                                                     | Choice criteria | Score criteria | Returned details                                                                         | Contract source                                                                                                                                                                  |
| ------------------------------------------------------------------ | --------------- | -------------- | ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Native Jev `jev-latest`                                            | 1 to 26         | 1 to 10        | Choice/Score probabilities and confidence; Noul yes-probability                          | [Native route and shared validator](../../crates/antiburn-local/src/analysis/jev.rs), [native capabilities](../../crates/antiburn-local/src/analysis/jev/capabilities.rs)        |
| Cloudflare Clef or Clef-flash                                      | 2 to 26         | 2 to 10        | Choice/Score probabilities and confidence; Noul yes-probability                          | [Cloudflare adapter](../../apps/desktop/src-tauri/src/jev_cloudflare.rs), [checked-in metadata](../../apps/desktop/src-tauri/tests/fixtures/cloudflare/clef-model-metadata.json) |
| Custom direct or Cloudflare envelope                               | 2 to 26         | 2 to 10        | Strict adapter contract: Choice/Score probabilities and confidence; Noul yes-probability | [`typed_adapter`](../../crates/antiburn-local/src/analysis/jev/capabilities.rs)                                                                                                  |
| Ollama `0.40.1`, accepted `tev1`                                   | 2 to 24         | 2 to 24        | Choice/Score probabilities and confidence; Noul yes-probability                          | [Verified output support and renderer guards](../../apps/desktop/src-tauri/src/jev_ollama.rs)                                                                                    |
| Ollama `0.40.1`, accepted `clef` or characterized generic encoding | 2 to 26         | 2 to 26        | Choice/Score probabilities and confidence; Noul yes-probability                          | [Verified output support and renderer guards](../../apps/desktop/src-tauri/src/jev_ollama.rs)                                                                                    |

YAML requires at least two Choice or Score criteria. Use the intersection of that
language rule, the question's type-specific output interval, and any stricter
selected numeric limit. Do not apply Jev's Score maximum to an accepted Ollama
profile. Unknown runner, encoding, or renderer metadata does not inherit another
profile's support. Numeric input limits do not establish output support.

The 26-criterion Choice limit is a local guard, not a universal upstream maximum.
Upstream documentation alone does not widen the local Jev output guards or the
accepted runner contract. Ollama's source pin is
[`cf2a313a298066d572c36812e5ad30a21c0db13b`](https://github.com/ollama/ollama/tree/cf2a313a298066d572c36812e5ad30a21c0db13b/decision)
(`0.40.1`). The Tev1 renderer narrows its general 26-criterion interval to 24.

## Answer validation

A response must contain exactly the requested named answers with matching types.
Choice must return a declared label and a complete map of its declared labels.
Score must return a finite number from zero through the last criterion index.
Its probability and legend keys must be exactly `0` through `n-1`; each legend
value must equal the criterion at that index. Keys such as `01` are invalid.
Noul is a finite yes-probability in `[0, 1]`, not a Boolean, and has no separate
confidence field.

Every probability and available confidence value must be finite and in `[0, 1]`.
Probability sums use absolute error `<= 0.02` under `f64` arithmetic. Score sums
follow criterion index order, `0` through `n-1`, not lexicographic wire-key order.
Choice sums follow label-map order. Preserve the raw values; do not normalize
them, replace a valid Choice label, or recompute a Score from its distribution.
Provider confidence is distinct from a selected label's probability. Do not use
one in place of the other. All five canonical definitions need label
probabilities for their 0.95 gates; a label-only response cannot run them.

## Request and accounting boundaries

Native Jev uses TypeSafe's `jev-latest` alias, currently `jev-1.14.0`, with the
saved credential. A new Jev release does not require a credential or profile
migration. Native requests send `TypeSafe-Danger-Zone-Long-Context: true`.
The local total and state-plus-longest-question ceilings are 262,000 estimated
tokens each, before a 4,096-token rendering reserve. The usable ceiling is
257,904 estimated tokens. The serialized body guard is 384 KiB; the resident
request-charge guard is 832 KiB. The shared 4 MiB request-charge budget and
separate 60-second admission and transport deadlines still apply. These are
local limits, not a promise to admit every 262k-token provider input.
See [TypeSafe models](https://docs.typesafe.ai/models.md) and the
[HTTP API](https://docs.typesafe.ai/api.md), checked on 2026-10-27.

Custom routes do not receive the native header or long-context limits. The exact
official direct TypeSafe Custom route recognizes the concrete 1.13 and 1.14
models with standard, header-free limits. Custom configurations keep their exact
endpoint and model selector. Without a supported capability source, they remain
saved but cannot run checks. The app does not guess limits or discovery URLs.

Settings manages limits without manual overrides. It uses provider discovery
where supported and maintained profiles elsewhere. New work replaces saved
manual limits; historical preparations remain unchanged. A changed execution
profile stops incompatible unfinished work without refitting or another send.

Cloudflare Clef uses a documented 65,536-token context. Clef-flash uses 24,576
tokens, as documented on the [Clef](https://developers.cloudflare.com/workers-ai/models/clef/)
and [Clef-flash](https://developers.cloudflare.com/workers-ai/models/clef-flash/)
pages checked on 2026-10-27. Keep their local profiles separate. These routes do
not promise immutable served releases or a universal latest-release policy.

Ollama uses the selected installed model and verified runtime metadata. It does
not download updates or select cloud-backed models as free local inference.
An installed `latest` tag does not prove the newest registry artifact. Unknown
or unsupported required metadata blocks check execution.
After slot admission, each load or inference checks the installed tag digest
against the discovered digest. A changed digest blocks the send without adopting
new metadata or refitting the request. A metadata failure sends no inference and
records no inference usage. The tag can still change after this check; the
response model name does not prove an immutable served digest.

One wire request can contain mixed question types. Each selected route also has
request-byte, response-byte, token, and question-count limits. Keep these limits
separate from verified output support. The local queue and transport memory caps
remain additional admission limits, not claims about provider capacity.

Usage accounting and answer validity are separate. A rejected answer can still
incur usage. Record unambiguous usage before answer validation. Ambiguous usage
must not select one duplicate value as the billed count. Neither a protocol pass
nor synthetic provider tests establish check quality or native source coverage.

Native alias responses can report a different, bounded model identifier. Other
routes keep exact response-model matching. The app keeps the requested alias in
immutable preparation and records the observed served model per attempt. An
alias echo does not establish a concrete release. Safe retries reuse the exact
saved alias request, but TypeSafe can serve a newer release between attempts.
Unknown delivery never permits an automatic resend.

Saved connection evidence separates that requested model from observed models
for individual decisions and attempts. Its attempt list contains at most 256
rows and marks a longer list as truncated. This display bound does not remove
stored observations or replace unknown versions with the current alias target.

Only observed native `jev-1.13.0` and `jev-1.14.0` usage has a known TypeSafe
price: $0.042 per million input tokens, with free output. Future or unidentified
models remain unpriced; confirmed token counts remain available. Custom and
Cloudflare usage does not receive TypeSafe pricing based on a model name.
Session deletion preserves incurred usage and its deduplication markers.
Clear index removes the markers, observed model metadata, and totals.
