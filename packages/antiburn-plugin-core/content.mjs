import { VERSION, CONTRACT_VERSION } from './core.mjs'

/** @param {string} host @param {string} invocation @param {string} name */
export function instructions(host, invocation, name = 'antiburn') {
  return `---
name: ${name}
${host === 'cursor' ? 'disable-model-invocation: true\n' : ''}description: Show antiburn shell help or check executable availability without launching the app.
---

# antiburn

Shell ${VERSION}; contract ${CONTRACT_VERSION}; host ${host}. This is an explicit, model-mediated command and may use model capacity. No automatic startup behavior is supplied.

Accept empty arguments or help, and status. Accept one optional --ascii or --flame presentation flag. Reject other arguments with: Usage: ${invocation} [help|status] [--ascii|--flame]. Never treat arguments as shell code.

On each explicit status request, resolve executable availability using permitted host tools. Help does not perform discovery. Do not inspect saved transcripts or persist session records. If tools are denied, unavailable, or unable to verify executable permissions, report CLI check unavailable. Do not ask for broader permissions just to run help.

Resolve the absolute ANTIBURN_EXECUTABLE environment value when supplied; otherwise search PATH for the antiburn executable. Use filesystem metadata and execution permission checks on POSIX; on Windows use executable PATHEXT files. Follow symbolic links to regular files. Bound the search to 128 PATH entries and a short timeout. Permission or incomplete-search errors mean unavailable; not found requires a complete search. Never interpolate environment paths or command arguments into shell code. Never execute antiburn, including --version, --help, status, or check. A shell alias, app bundle, directory, or nonexecutable file does not establish availability.

Do not display CLI discovery notices automatically or above help. Show availability only for an explicit status request.

For help, show the usage line and explain that this shell checks executable availability without launching antiburn. For status, render the brand heading as Markdown above a fenced text code block containing only the CLI and diagnostic rows. Keep the heading outside the block so the inline code delimiters render correctly. Preserve each diagnostic row on its own line. Replace the CLI value only with verified Found, Not found, or Check unavailable:

🔥 \`antiburn\`
CLI: Check unavailable
App connection: Not checked
Shell: ${VERSION} (contract ${CONTRACT_VERSION})
Host: ${host}; version unknown
Invocation: ${invocation} (model-mediated)
Runtime certification: Unverified

Use 🔥 \`antiburn\` by default. Preserve literal backtick delimiters when Markdown is unavailable. Use \`antiburn\` without the emoji with --ascii or an ASCII-only surface. The --flame flag remains an alias for the default. Never output ANSI escapes or claim Connected. No native color or glyph detection is available. Do not claim the candidate's publisher, version, app state, or protocol compatibility.

Do not launch the app, contact a daemon or remote endpoint, collect credentials, read transcripts, poll, install software, or change settings. The optional CLI is not required for help. If missing, explain that an executable named antiburn must be exposed to this host's PATH; installing an app bundle alone may not provide it. Do not invent a CLI installer.
`
}
