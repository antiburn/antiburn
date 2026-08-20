// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(
  new URL("../.github/workflows/ci.yml", import.meta.url),
  "utf8",
);

function namedStep(name) {
  const marker = `      - name: ${name}`;
  const start = workflow.indexOf(marker);
  assert.notEqual(start, -1, `${name} step must exist`);
  const next = workflow.indexOf("\n      - name:", start + marker.length);
  return workflow.slice(start, next === -1 ? undefined : next);
}

test("the cross-platform release cache target is shell-neutral", () => {
  const step = namedStep(
    "Compile the release target without bundling or signing",
  );

  assert.match(step, /--target "\$\{\{ matrix\.target \}\}" --no-bundle/);
  assert.doesNotMatch(step, /\$TARGET/);
});

test("the standalone HUD crate has dedicated backend checks", () => {
  assert.match(workflow, /apps\/desktop\/src-tauri\/crates\/hud -> target/);

  for (const [name, command] of [
    ["Check HUD crate formatting", "cargo fmt --check"],
    ["Clippy HUD crate", "cargo clippy --all-targets --locked -- -D warnings"],
    ["Test HUD crate", "cargo test --locked"],
  ]) {
    const step = namedStep(name);
    assert.match(
      step,
      /working-directory: apps\/desktop\/src-tauri\/crates\/hud/,
    );
    assert.ok(step.includes(`run: ${command}`));
  }
});
