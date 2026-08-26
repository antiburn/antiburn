// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import test from "node:test";

const root = resolve(import.meta.dirname, "..");
const installer = join(root, "install.sh");

function executable(path, contents) {
  writeFileSync(path, contents, { mode: 0o755 });
  chmodSync(path, 0o755);
}

function linkCommands(bin, commands) {
  for (const command of commands) {
    const path = execFileSync("sh", ["-c", `command -v ${command}`], {
      encoding: "utf8",
    }).trim();
    symlinkSync(path, join(bin, command));
  }
}

function harness({
  os = "Linux",
  arch = "x86_64",
  checksum = "a".repeat(64),
  packageType = "appimage",
} = {}) {
  const directory = mkdtempSync(join(tmpdir(), "antiburn-install-test-"));
  const bin = join(directory, "bin");
  const home = join(directory, "home");
  const log = join(directory, "commands.log");
  mkdirSync(bin);
  mkdirSync(home);
  linkCommands(bin, ["awk", "chmod", "id", "ln", "mkdir", "mktemp", "mv", "rm"]);

  executable(
    join(bin, "uname"),
    `#!/bin/sh\n[ "\${1:-}" = "-m" ] && printf '%s\\n' '${arch}' || printf '%s\\n' '${os}'\n`,
  );
  executable(
    join(bin, "curl"),
    `#!/bin/sh
output=''
url=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --output) output="$2"; shift 2 ;;
    --write-out) shift 2 ;;
    --*) shift ;;
    *) url="$1"; shift ;;
  esac
done
if [ "$output" = "/dev/null" ]; then
  printf '%s' 'https://github.com/antiburn/antiburn/releases/tag/antiburn-v1.2.3'
elif [ "\${url##*/}" = "SHA256SUMS" ]; then
  printf '%s  %s\\n' '${checksum}' '${packageType === "deb" ? "antiburn_1.2.3_amd64.deb" : "antiburn_1.2.3_amd64.AppImage"}' > "$output"
else
  printf '%s' 'release asset' > "$output"
fi
`,
  );
  executable(
    join(bin, "sha256sum"),
    `#!/bin/sh\nprintf '%s  %s\\n' '${checksum}' "$1"\n`,
  );

  if (packageType === "deb") {
    executable(
      join(bin, "apt-get"),
      '#!/bin/sh\nprintf \'%s\\n\' "$*" >> "$TEST_COMMAND_LOG"\n',
    );
    executable(
      join(bin, "dpkg-deb"),
      `#!/bin/sh
case "$3" in
  Package) printf '%s\\n' antiburn ;;
  Architecture) printf '%s\\n' amd64 ;;
  Version) printf '%s\\n' 1.2.3 ;;
  *) exit 1 ;;
esac
`,
    );
    executable(join(bin, "sudo"), '#!/bin/sh\nexec "$@"\n');
  }

  return {
    directory,
    home,
    log,
    env: {
      ...process.env,
      ANTIBURN_VERSION: "1.2.3",
      HOME: home,
      PATH: bin,
      TEST_COMMAND_LOG: log,
      TMPDIR: directory,
    },
  };
}

test("install.sh has valid POSIX shell syntax", () => {
  execFileSync("sh", ["-n", installer]);
});

test("install.sh keeps execution on its final line", () => {
  const source = readFileSync(installer, "utf8");
  assert.equal(source.trimEnd().split("\n").at(-1), 'install_antiburn "$@"');
});

test("install.sh rejects an unsupported operating system before downloading", () => {
  const context = harness({ os: "Plan9" });
  const result = spawnSync("/bin/sh", [installer], {
    encoding: "utf8",
    env: context.env,
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Unsupported operating system: Plan9/);
  assert.doesNotMatch(result.stdout, /Downloading antiburn_/);
});

test("install.sh installs the verified AppImage without root", () => {
  const context = harness();
  const result = spawnSync("/bin/sh", [installer], {
    encoding: "utf8",
    env: context.env,
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Verified SHA-256/);
  assert.ok(existsSync(join(context.home, "Applications", "antiburn.AppImage")));
  assert.ok(existsSync(join(context.home, ".local", "bin", "antiburn")));
});

test("install.sh uses APT for a verified Debian package", () => {
  const context = harness({ packageType: "deb" });
  const result = spawnSync("/bin/sh", [installer], {
    encoding: "utf8",
    env: context.env,
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(
    readFileSync(context.log, "utf8"),
    /install --yes --allow-downgrades .*antiburn_1\.2\.3_amd64\.deb/,
  );
});

test("install.sh stops before installation when the checksum differs", () => {
  const context = harness({ checksum: "a".repeat(64) });
  executable(
    join(context.directory, "bin", "sha256sum"),
    `#!/bin/sh\nprintf '%s  %s\\n' '${"b".repeat(64)}' "$1"\n`,
  );
  const result = spawnSync("/bin/sh", [installer], {
    encoding: "utf8",
    env: context.env,
  });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Checksum verification failed/);
  assert.equal(existsSync(join(context.home, "Applications", "antiburn.AppImage")), false);
});

test("a truncated install.sh does not start installation", () => {
  const context = harness();
  const truncated = join(context.directory, basename(installer));
  const lines = readFileSync(installer, "utf8").trimEnd().split("\n");
  writeFileSync(truncated, `${lines.slice(0, -1).join("\n")}\n`);
  const result = spawnSync("/bin/sh", [truncated], {
    encoding: "utf8",
    env: context.env,
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(existsSync(join(context.home, "Applications", "antiburn.AppImage")), false);
});

test("install.sh contains the required macOS trust checks", () => {
  const source = readFileSync(installer, "utf8");
  assert.match(source, /hdiutil verify/);
  assert.match(source, /codesign --verify --deep --strict/);
  assert.match(source, /spctl --assess --type execute/);
  assert.match(source, /ai\.antiburn\.desktop/);
});

test("the application release publishes both root installers before checksums", () => {
  const workflow = readFileSync(join(root, ".github", "workflows", "release-app.yml"), "utf8");
  const copyIndex = workflow.indexOf("cp install.sh install.ps1 dist/");
  const checksumIndex = workflow.indexOf('mv "${RUNNER_TEMP}/SHA256SUMS" dist/SHA256SUMS');
  assert.ok(copyIndex > 0);
  assert.ok(checksumIndex > copyIndex);
  assert.match(workflow, /require_count 'install\.sh' 1/);
  assert.match(workflow, /require_count 'install\.ps1' 1/);
});
