# kiro_crew_bundle

A representative Kiro Crew session log (`~/.kiro/crew/sessions/<name>.jsonl`).

`session.jsonl` — first line is a `{"_type":"metadata",…}` header carrying the
session `model` (`safe-model` here), followed by `{"role":…,"content":…}`
message records. Two assistant records mean the reader emits two model-identity
events. Crew logs carry no per-turn token usage, so no `Usage` is recorded.
