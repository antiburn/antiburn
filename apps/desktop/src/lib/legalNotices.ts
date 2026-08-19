// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The licence and notice bodies are imported from the repository's own
// canonical files rather than copied into source: NOTICE is exempt from the
// whole-tree source-boundary scan by path, and a copy of its text in a
// scanned file would fail CI. `?raw` keeps a single source of truth and
// inlines the string at build time.
import licenseText from "../../../../LICENSE?raw"
import noticeText from "../../../../NOTICE?raw"

/** The full MPL-2.0 text the application ships under. */
export const LICENSE_TEXT: string = licenseText

/** The repository's NOTICE file: copyright and provenance statement. */
export const NOTICE_TEXT: string = noticeText

/**
 * Third-party attributions. Static by design: the application bundles very
 * little third-party *content* (as opposed to code, which the SBOMs inventory
 * at release time), and each piece that needs a name is named here.
 */
export const ATTRIBUTIONS: ReadonlyArray<{ title: string; body: string }> = [
  {
    title: "Agent brand marks",
    body:
      "Agent rows render brand marks from the simple-icons package, and " +
      "OpenAI’s mark from Gil Barbara’s SVG Logos. Both publish their path " +
      "data under CC0-1.0, and where a mark is drawn in a brand’s own colour " +
      "that colour is the value those packages record. The marks remain " +
      "trademarks of their respective owners and appear here nominatively — " +
      "to say which tool a session belongs to, not to claim any affiliation.",
  },
  {
    title: "Provider artwork",
    body:
      "Model providers appear as letter tiles rather than logos: their " +
      "artwork is licensed separately from this application and is not " +
      "bundled with it.",
  },
]
