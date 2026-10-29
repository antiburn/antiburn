# Main 64 receipt fixtures

These fixtures contain synthetic inputs and raw JSON from the native receipt
encoder at antiburn commit `143d4b05681938cb362e828ecc27d2e082638176`.
They describe that exact source producer, not a published release range.

The capture tests run in a detached worktree of that commit. They call the old
native file and live journal readers, then serialize their returned cursors.
They do not change cursor revision numbers or replace nested parser bodies.
The capture harness is kept in `producer.patch` for reproduction.

Large integer arrays use a lossless sparse-array storage codec to keep the JSON
small. `decode.rs` expands them before decoding. `cursor_sha256` records the
old encoder's full JSON before that storage codec runs. Tests check the digest
before rebinding the temporary file identity.

File fixtures retain a machine-local synthetic file identity. Continuation tests
replace only `anchor.identity` with the identity of the temporary file containing
the exact captured prefix. Hashes, positions, framing bytes, and parser state
remain unchanged. Separate tests reject changed source bytes and bindings.

No fixture contains user data, credentials, or provider responses.

Capture commands, from a detached worktree of the pinned commit after applying
`producer.patch`:

```sh
ANTIBURN_RECEIPT_FIXTURE_OUTPUT=/absolute/output/directory \
cargo nextest run --manifest-path crates/antiburn-local/Cargo.toml \
  --lib --test opencode_characterization -E 'test(capture_main_64_)'
```

The three ordinary file fixtures retain one completed result, one unresolved
call, and a partial result line. The fourth retains one queued result after the
old 256-result page bound. The journal retains one terminal and one running part.

`journal-v1-evicted-context.json` retains 17 terminal parts and one running part
at sequence 38. Each terminal part freezes a different synthetic request. The
old encoder keeps only contexts 1 through 16, but part 0 still references the
evicted context 0. `next_context` is 17. The fixture also records four suffix
events and the old reader's three results: a correction with an evicted context,
a correction with a retained context, and completion of the running part after
a request text update. Tests compare the frozen contexts with these old results.

To capture only this fixture, use the same command with
`--test opencode_characterization -E 'test(capture_main_64_journal_eviction_receipt)'`
instead of the broader test selection. The production encoder remains unchanged.
