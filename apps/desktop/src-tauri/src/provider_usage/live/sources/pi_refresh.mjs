// Delegate one OAuth refresh to Pi's own SDK.
//
// argv[2] is the resolved pi package entry file; argv[3] is the provider id
// ("anthropic" or "openai-codex"). `ModelRuntime.getAuth` refreshes an
// expired OAuth token inside Pi's own `CredentialStore.modify` — a
// serialized read-modify-write under a cross-process file lock — and Pi
// itself persists the rotated credential to auth.json. This script makes no
// model request and spends no tokens.
import { pathToFileURL } from "node:url";

const { ModelRuntime } = await import(pathToFileURL(process.argv[2]).href);
const runtime = await ModelRuntime.create();
const result = await runtime.getAuth(process.argv[3]);
console.log(JSON.stringify({ ok: result != null }));
// Exit explicitly: the runtime can hold handles that keep node alive.
process.exit(0);
