// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import "@testing-library/jest-dom/vitest"
import { cleanup } from "@testing-library/react"
import { afterEach } from "vitest"

// Several label helpers format a wall-clock time, which the machine's zone
// decides. Without a fixed zone an assertion passes in one timezone and fails
// in the next, so the suite pins one.
process.env.TZ = "UTC"

afterEach(() => {
  cleanup()
})
