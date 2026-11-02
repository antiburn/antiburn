import { mkdir, writeFile } from "node:fs/promises"
import path from "node:path"

/** Generate the local fixture. Its YAML and image stay outside version control. */
export async function createPromotionFixture(
  directory = path.resolve(process.cwd(), "../../.agent-artifacts/promotions"),
): Promise<string> {
  const folder = path.join(directory, "dummy-promo")
  await mkdir(path.join(folder, "assets"), { recursive: true })
  await writeFile(
    path.join(folder, "promo.yaml"),
    "schema_version: 1\nid: dummy-promo\nrevision: 1\ntitle: Sample promotion\ndevelopment_only: true\n\nscreens:\n  announcement:\n    eyebrow: Test promotion\n    title: A different feature\n    description: Test content for the reusable promotion panel.\n    media:\n      kind: image\n      src: ./assets/dummy-promo.svg\n      alt: Sample feature illustration.\n    features:\n      - icon: gauge\n        title: Different content\n        description: Rendered from YAML without feature-specific code.\n    primary_action:\n      label: View usage\n      type: app\n      target: settings\n      pane: usage\n",
  )
  await writeFile(
    path.join(folder, "assets/dummy-promo.svg"),
    '<svg xmlns="http://www.w3.org/2000/svg" width="800" height="416" viewBox="0 0 800 416">\n  <rect width="800" height="416" fill="#171717" />\n  <rect x="88" y="92" width="624" height="88" rx="16" fill="#292929" />\n  <rect x="88" y="206" width="624" height="118" rx="16" fill="#222222" />\n  <circle cx="126" cy="136" r="12" fill="#ff642d" />\n  <rect x="154" y="127" width="164" height="18" rx="9" fill="#dedede" />\n  <rect x="118" y="240" width="500" height="10" rx="5" fill="#525252" />\n  <rect x="118" y="270" width="392" height="10" rx="5" fill="#ff642d" />\n</svg>\n',
  )
  return directory
}
