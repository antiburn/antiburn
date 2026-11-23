# Remote sessions

Antiburn Desktop has retired SSH session collection. The app no longer
connects to SSH hosts, downloads session copies, or shows the remote host
settings and source filter. The release workflow still builds and packages
the Linux helper while its release tooling remains in place.

## Upgrade cleanup

Before session workers or windows start, Antiburn removes previously synced SSH
sessions and their owned analysis and evidence from its local database. It also
removes cached transcripts, staging files, and SSH inventory snapshots from the
app's known SSH cache locations. If cached files cannot be removed, Antiburn logs
the failure, continues startup, and retries file cleanup on the next launch.
SSH database cleanup must succeed before startup continues. Cleanup can safely
retry after an interrupted launch and does not need a saved migration flag.

Local and WSL sessions, preferences, scan folders, and repository choices remain
unchanged. Cleanup does not follow transcript paths stored in database rows or
cache symlinks to files elsewhere. Remote originals are untouched. Antiburn does
not connect to another computer or uninstall a helper left there.

Saved SSH sessions are no longer available in the app. See
[session coverage](session-coverage.md) and [check coverage](check-coverage.md) for
current supported sources and evidence contracts.
