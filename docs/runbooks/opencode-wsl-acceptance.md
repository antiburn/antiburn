# OpenCode WSL stored-session acceptance

The Windows x64 engine job runs the production stored-session bridge through real
`wsl.exe` processes, mounted UNC paths, and Linux curl. Its HTTP service is
synthetic and follows the public `SessionTransfer.Data` shape at OpenCode
`v2.0.32`, commit `843a6c514e606b436a7fe713422ace0a93ad3d61`.
This test does not prove stock-producer behavior, WSL live checks, or WSL 1.

## Run in CI

Run the existing **CI** workflow on the candidate branch:

```sh
gh workflow run ci.yml --ref <branch>
```

Manual dispatch selects all native Rust platforms, including Windows x64 and
ARM64, Linux x64 and ARM64, and macOS. The WSL step runs on Windows x64 when the
engine workspace is selected. A draft PR normally runs only Linux. A `ci:full`
label adds native Windows and macOS, but not the full ARM64 matrix. Do not use
a reduced draft run as final platform evidence.

The WSL step calls `scripts/run-opencode-wsl-acceptance.ps1`. It fails if WSL 2,
package setup, mounted discovery, authentication, transfer, parsing, or cleanup
fails. It does not turn a missing prerequisite into a successful skip. The Rust
test is ignored in ordinary suites because it requires a new disposable distro.
The CI script selects only this ignored test, not live-model evals.

## Isolation and prerequisites

Provisioning runs only on a GitHub-hosted Windows x64 runner. The script imports
a new `AntiburnAcceptance-<random hex>` WSL 2 distro. It does not reuse an existing
distro, explicitly change the WSL default, install OpenCode, or run a producer command.
WSL can make the first imported distro the default on an empty runner.
The script unregisters only its owned distro in `finally`.

The image and package choices are exact:

| Input                   | Pin                                                                    |
| ----------------------- | ---------------------------------------------------------------------- |
| Ubuntu base image       | `ubuntu-base-24.04.5-base-amd64.tar.gz`                                |
| Image SHA-256           | `e77b6f10c2590cef872b33ee9f635a0e3fd1f57fb074c0e52b5c7f56147a0c86`     |
| CA bootstrap package    | `ca-certificates=20240203`, Ubuntu `noble main` release archive        |
| Ubuntu package snapshot | `20261020T000000Z`, `noble`, `noble-updates`, `noble-security`, `main` |
| curl                    | `8.5.0-2ubuntu10.16`                                                   |
| python3 package         | `3.12.3-0ubuntu2.1`                                                    |

The image downloads over HTTPS and must match its checksum. The minimal image
has no CA certificates. Apt first installs the pinned CA package and its
dependencies from `http://archive.ubuntu.com/ubuntu`, using signed Ubuntu metadata.
The script then replaces that source with the fixed HTTPS snapshot for curl,
Python, and their dependencies. It keeps package authentication and TLS
verification enabled. Both metadata updates fail on any download error. The
snapshot metadata expiry check is off because the test must remain reproducible
after the snapshot date. These packages are CI test tools, not product
dependencies.

The script creates a new `antiburn` user and HOME inside that distro. It restarts
only that distro, then checks its default user without a user override. Commands
use `wsl.exe --exec` so a Linux shell cannot expand curl arguments. The test
starts one Python loopback HTTP service as that user. The service creates a
random password in a mode-0600 registration under that HOME and removes it at
shutdown. Neither the test nor its output records the password or headers.
The Linux service exits after at most 120 seconds even if the test host stops.
No real user registration, transcript, database, or service is used.

## Evidence and resource limits

The test requires exactly four authenticated GET requests: health, list, export,
and health again. It also requires one V1 metadata probe after the V2 read so
historical V1 sessions remain discoverable. A synthetic CLI accepts only the
exact metadata SQL query, `--pure`, JSON output, and the four production flags
that disable updates, model fetches, default plugins, and migrations. It returns
empty V1 history and records the allowed arguments. All other commands fail and
leave a rejection marker, including service startup, API calls, installation,
exports, and SQL writes. No real OpenCode executable runs. The test checks that
the registration bytes do not change and that the service stops and deletes its
credential file. Portable fixture tests check the allowlist and its rejections.

The transfer contains two synthetic messages, one stored read-tool record, UTF-8
text, and a `262,161`-byte tool output. This crosses normal pipe buffer sizes and
tests complete Windows-to-Linux stdin and Linux-to-Windows stdout transfer.
The production reader must keep the stored tool without granting native live
result eligibility. Passing output records request count, response bytes, and
elapsed bridge time. These are test measurements, not a general performance
promise.

Provisioning downloads a `30,028,293`-byte Ubuntu base archive, the CA bootstrap,
and the fixed curl/Python package closure. It uses runner disk for the extracted
distro, package cache, and Rust build outputs. WSL 2 also starts its Linux VM.
This is more work than the normal synthetic unit suite. Setup has a five-minute
deadline; the CI step has a 15-minute deadline. The bridge has a 60-second batch
deadline and the test waits at most 30 seconds for the service and 10 seconds for
its shutdown.

The test does not measure sustained CPU, RSS, large-history retention, many live
sessions, stream stalls, or heartbeat behavior. Use the native resource and
lifecycle tests for those claims. Keep stock-producer smoke evidence separate
from this synthetic-service transport test.
