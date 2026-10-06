import assert from "node:assert/strict";
import { test } from "node:test";
import {
  caskVersion,
  updateCask,
  versionFromTag,
} from "./update-homebrew-cask.mjs";

const arm = "a".repeat(64);
const intel = "b".repeat(64);
const oldHash = "c".repeat(64);
const tag = "antiburn-v2.10.0";
const release = {
  tagName: tag,
  isDraft: false,
  isPrerelease: false,
  assets: [
    "antiburn_2.10.0_aarch64.dmg",
    "antiburn_2.10.0_x64.dmg",
    "SHA256SUMS",
  ].map((name) => ({ name })),
};
const sums = `${arm}  antiburn_2.10.0_aarch64.dmg\n${intel} *antiburn_2.10.0_x64.dmg\n`;
const cask = `cask "antiburn" do
  arch arm: "aarch64", intel: "x64"
  version "2.9.0"
  sha256 arm:   "${oldHash}",
         intel: "${oldHash}"
  auto_updates true
  app "antiburn.app"
  zap trash: "~/Library/Logs/antiburn"
end
`;

test("updates a numeric version and both architecture hashes, preserving policy", () => {
  const updated = updateCask(cask, tag, release, sums);
  assert.equal(
    updated,
    cask
      .replace('version "2.9.0"', 'version "2.10.0"')
      .replace(oldHash, arm)
      .replace(oldHash, intel),
  );
  assert.equal(caskVersion(updated), "2.10.0");
  assert.equal(updateCask(updated, tag, release, sums), updated);
});

test("accepts CRLF checksum manifests", () => {
  assert.equal(
    updateCask(cask, tag, release, sums.replaceAll("\n", "\r\n")),
    updateCask(cask, tag, release, sums),
  );
});

test("rejects engine tags, prerelease tags, and malformed desktop tags", () => {
  for (const invalid of [
    "antiburn-local-v2.10.0",
    "antiburn-v2.10.0-rc.1",
    "v2.10.0",
    "antiburn-v2.10",
    "antiburn-v2.10.0\n",
  ]) {
    assert.throws(() => versionFromTag(invalid), /stable desktop tag/);
  }
});

test("requires a published stable release with an exact matching tag", () => {
  for (const patch of [
    { isDraft: true },
    { isPrerelease: true },
    { tagName: "antiburn-v2.11.0" },
    { isDraft: undefined },
  ]) {
    assert.throws(
      () => updateCask(cask, tag, { ...release, ...patch }, sums),
      /published, stable/,
    );
  }
});

test("rejects missing and duplicate required assets", () => {
  for (const asset of release.assets) {
    assert.throws(
      () =>
        updateCask(
          cask,
          tag,
          {
            ...release,
            assets: release.assets.filter((value) => value !== asset),
          },
          sums,
        ),
      /release asset/,
    );
    assert.throws(
      () =>
        updateCask(
          cask,
          tag,
          { ...release, assets: [...release.assets, asset] },
          sums,
        ),
      /release asset/,
    );
  }
});

test("rejects missing, duplicate, and malformed required checksums", () => {
  for (const invalid of [
    sums.split("\n")[0],
    `${sums}${sums}`,
    sums.replace(arm, "not-a-hash"),
    sums.replace(intel, "b".repeat(63)),
  ]) {
    assert.throws(
      () => updateCask(cask, tag, release, invalid),
      /SHA-256 checksum/,
    );
  }
});

test("rejects a version regression and changed hashes for an existing version", () => {
  assert.throws(
    () => updateCask(cask.replace("2.9.0", "2.11.0"), tag, release, sums),
    /downgrade/,
  );
  assert.throws(
    () => updateCask(cask.replace("2.9.0", "2.10.0"), tag, release, sums),
    /checksums changed/,
  );
});

test("rejects ambiguous or changed cask structure", () => {
  for (const invalid of [
    cask.replace('version "2.9.0"', "version :latest"),
    `${cask}  version "2.9.0"\n`,
    cask.replace("sha256 arm:", "sha256 intel:"),
    `${cask}  sha256 :no_check\n`,
  ]) {
    assert.throws(
      () => updateCask(invalid, tag, release, sums),
      /Expected exactly one/,
    );
  }
});
