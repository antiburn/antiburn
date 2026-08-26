// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const entitlements = readFileSync(
  new URL("../apps/desktop/src-tauri/Entitlements.plist", import.meta.url),
  "utf8",
);
const releaseConfig = JSON.parse(
  readFileSync(
    new URL(
      "../apps/desktop/src-tauri/tauri.release.conf.json",
      import.meta.url,
    ),
    "utf8",
  ),
);
const desktopIgnore = readFileSync(
  new URL("../apps/desktop/.gitignore", import.meta.url),
  "utf8",
);
const workflow = readFileSync(
  new URL("../.github/workflows/release-app.yml", import.meta.url),
  "utf8",
);

test("the macOS entitlement names the provisioned app identity", () => {
  assert.match(
    entitlements,
    /<key>com\.apple\.application-identifier<\/key>\s*<string>JCK9YYRR88\.ai\.antiburn\.desktop<\/string>/,
  );
  assert.match(
    entitlements,
    /<key>com\.apple\.developer\.team-identifier<\/key>\s*<string>JCK9YYRR88<\/string>/,
  );
  assert.match(
    entitlements,
    /<key>com\.apple\.developer\.usernotifications\.communication<\/key>\s*<true\/>/,
  );
});

test("the release overlay embeds only the ignored provisioning profile", () => {
  assert.deepEqual(releaseConfig, {
    bundle: {
      macOS: {
        files: {
          "embedded.provisionprofile": "antiburn.provisionprofile",
        },
      },
    },
  });
  assert.match(desktopIgnore, /^src-tauri\/antiburn\.provisionprofile$/m);
});

test("the release workflow validates and verifies the profile-backed entitlement", () => {
  assert.match(workflow, /secrets\.APPLE_PROVISIONING_PROFILE/);
  assert.match(workflow, /security cms -D -i "\$profile"/);
  assert.match(workflow, /ProvisionsAllDevices/);
  assert.match(workflow, /ExpirationDate/);
  assert.match(
    workflow,
    /TAURI_SIGNING_CONFIG=apps\/desktop\/src-tauri\/tauri\.release\.conf\.json/,
  );
  assert.match(
    workflow,
    /test -f "\$app\/Contents\/embedded\.provisionprofile"/,
  );
  assert.match(workflow, /codesign -d --entitlements :- "\$executable"/);
  assert.match(workflow, /test "\$communication" = "true"/);
  assert.match(
    workflow,
    /test ! -e "\$app\/Contents\/embedded\.provisionprofile"/,
  );
});
