// Tests for scripts/bump-version.mjs.
//
//   node --test scripts/bump-version.test.mjs
//   npm run test:version-script
//
// Every case builds a throwaway tree in the OS temp directory and points the
// script at it with MSCOUT_VERSION_ROOT, so the repository's own version files
// are never read as a fixture and never written.

import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import { test } from "node:test";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "bump-version.mjs");
const VERSION = "0.6.0";

// Minimal but realistic copies of the five stored locations plus a changelog
// with a pending entry and the compare/release link layout the repo uses.
const FIXTURES = {
  "src-tauri/Cargo.toml": [
    "[package]",
    'name = "mscout"',
    `version = "${VERSION}"`,
    'edition = "2021"',
    "",
    "[dependencies]",
    'serde = "1"',
    "",
  ].join("\n"),
  "src-tauri/Cargo.lock": [
    "version = 4",
    "",
    "[[package]]",
    'name = "serde"',
    'version = "1.0.200"',
    "",
    "[[package]]",
    'name = "mscout"',
    `version = "${VERSION}"`,
    "dependencies = [",
    ' "serde",',
    "]",
    "",
  ].join("\n"),
  "package.json": [
    "{",
    '  "name": "mscout",',
    `  "version": "${VERSION}",`,
    '  "private": true',
    "}",
    "",
  ].join("\n"),
  "package-lock.json": [
    "{",
    '  "name": "mscout",',
    `  "version": "${VERSION}",`,
    '  "lockfileVersion": 3,',
    '  "packages": {',
    '    "": {',
    '      "name": "mscout",',
    `      "version": "${VERSION}",`,
    '      "devDependencies": {}',
    "    }",
    "  }",
    "}",
    "",
  ].join("\n"),
  "src-tauri/tauri.conf.json": [
    "{",
    '  "productName": "MScout",',
    `  "version": "${VERSION}",`,
    '  "identifier": "com.mscout.app"',
    "}",
    "",
  ].join("\n"),
  "CHANGELOG.md": [
    "# Changelog",
    "",
    "## [Unreleased]",
    "",
    "### Added",
    "- A pending entry.",
    "",
    "## [0.5.0] - 2026-01-01",
    "",
    "### Added",
    "- An older entry.",
    "",
    "[Unreleased]: https://github.com/xinye83/mscout/compare/v0.5.0...HEAD",
    "[0.5.0]: https://github.com/xinye83/mscout/releases/tag/v0.5.0",
    "",
  ].join("\n"),
};

const VERSION_FILES = Object.keys(FIXTURES).filter((file) => file !== "CHANGELOG.md");

function tree(t, overrides = {}) {
  const root = mkdtempSync(join(tmpdir(), "mscout-version-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  for (const [file, text] of Object.entries({ ...FIXTURES, ...overrides })) {
    const path = join(root, file);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, text);
  }
  return root;
}

function run(root, args) {
  const result = spawnSync(process.execPath, [SCRIPT, ...args], {
    env: { ...process.env, MSCOUT_VERSION_ROOT: root },
    encoding: "utf8",
  });
  return { code: result.status, out: `${result.stdout}${result.stderr}` };
}

function snapshot(root, files = Object.keys(FIXTURES)) {
  return Object.fromEntries(files.map((file) => [file, readFileSync(join(root, file), "utf8")]));
}

function assertUnchanged(root, before) {
  assert.deepEqual(snapshot(root, Object.keys(before)), before);
}

test("--check passes when every location agrees", (t) => {
  const root = tree(t);
  const result = run(root, ["--check"]);
  assert.equal(result.code, 0, result.out);
  assert.match(result.out, /6 version locations agree on 0\.6\.0/);
});

test("--check reports drift and exits non-zero", (t) => {
  const root = tree(t, {
    "src-tauri/tauri.conf.json": '{\n  "productName": "MScout",\n  "version": "0.5.0",\n  "identifier": "com.mscout.app"\n}\n',
  });
  const result = run(root, ["--check"]);
  assert.equal(result.code, 1, result.out);
  assert.match(result.out, /src-tauri\/tauri\.conf\.json\s+app version\s+found 0\.5\.0, expected 0\.6\.0/);
  assert.match(result.out, /1 of 6 version locations disagree with 0\.6\.0/);
});

test("a write repairs only the drifted file", (t) => {
  const root = tree(t, {
    "src-tauri/tauri.conf.json": '{\n  "productName": "MScout",\n  "version": "0.5.0",\n  "identifier": "com.mscout.app"\n}\n',
  });
  const before = snapshot(root);
  const result = run(root, [VERSION]);
  assert.equal(result.code, 0, result.out);
  assert.equal(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8").includes(`"version": "${VERSION}"`), true);
  for (const file of VERSION_FILES.filter((name) => name !== "src-tauri/tauri.conf.json")) {
    assert.equal(readFileSync(join(root, file), "utf8"), before[file], `${file} was rewritten`);
  }
});

test("a full write changes only the version lines", (t) => {
  const root = tree(t);
  const before = snapshot(root, VERSION_FILES);
  const result = run(root, ["0.7.0"]);
  assert.equal(result.code, 0, result.out);

  // Cargo.toml, Cargo.lock, package.json, tauri.conf.json: one line each.
  // package-lock.json: root version plus the workspace package version.
  const expected = {
    "src-tauri/Cargo.toml": 1,
    "src-tauri/Cargo.lock": 1,
    "package.json": 1,
    "package-lock.json": 2,
    "src-tauri/tauri.conf.json": 1,
  };
  for (const [file, changes] of Object.entries(expected)) {
    const oldLines = before[file].split("\n");
    const newLines = readFileSync(join(root, file), "utf8").split("\n");
    assert.equal(newLines.length, oldLines.length, `${file} line count changed`);
    const differing = oldLines.filter((line, index) => line !== newLines[index]);
    assert.equal(differing.length, changes, `${file} changed ${differing.length} lines`);
    for (const line of differing) assert.match(line, /version/);
  }

  // The unrelated Cargo.lock dependency entries keep their own versions.
  const lock = readFileSync(join(root, "src-tauri/Cargo.lock"), "utf8");
  assert.match(lock, /name = "serde"\nversion = "1\.0\.200"/);
  assert.match(lock, /name = "mscout"\nversion = "0\.7\.0"/);

  assert.equal(run(root, ["--check"]).code, 0);
});

test("a reordered workspace package block fails closed without writing", (t) => {
  const root = tree(t, {
    "package-lock.json": [
      "{",
      '  "name": "mscout",',
      `  "version": "${VERSION}",`,
      '  "lockfileVersion": 3,',
      '  "packages": {',
      '    "": {',
      `      "version": "${VERSION}",`,
      '      "name": "mscout"',
      "    }",
      "  }",
      "}",
      "",
    ].join("\n"),
  });
  const before = snapshot(root);
  const result = run(root, ["0.7.0"]);
  assert.equal(result.code, 1, result.out);
  assert.match(result.out, /package-lock\.json: could not find the workspace package version in the expected shape/);
  assertUnchanged(root, before);
});

test("a broken JSON file aborts before any file is written", (t) => {
  // The locator still matches; only JSON.parse rejects the file. This is the
  // two-phase-write guarantee: no partial bump.
  const root = tree(t, {
    "src-tauri/tauri.conf.json": '{\n  "productName": "MScout",\n  "version": "0.6.0",\n}\n',
  });
  const before = snapshot(root);
  const result = run(root, ["0.7.0"]);
  assert.equal(result.code, 1, result.out);
  assert.match(result.out, /src-tauri\/tauri\.conf\.json: the original text is not valid JSON/);
  assertUnchanged(root, before);
});

test("a backwards move is refused unless --force is passed", (t) => {
  const root = tree(t);
  const before = snapshot(root, VERSION_FILES);
  const refused = run(root, ["0.5.0"]);
  assert.equal(refused.code, 1, refused.out);
  assert.match(refused.out, /refusing to move backwards: 0\.6\.0 -> 0\.5\.0/);
  assertUnchanged(root, before);

  const forced = run(root, ["0.5.0", "--force"]);
  assert.equal(forced.code, 0, forced.out);
  assert.equal(run(root, ["--check"]).code, 0);
});

test("--dry-run reports the edits and writes nothing", (t) => {
  const root = tree(t);
  const before = snapshot(root);
  const result = run(root, ["0.7.0", "--dry-run"]);
  assert.equal(result.code, 0, result.out);
  assert.match(result.out, /dry run: no files were written/);
  assertUnchanged(root, before);
});

test("--changelog promotes even when every version already matches", (t) => {
  const root = tree(t);
  const before = snapshot(root, VERSION_FILES);
  const result = run(root, [VERSION, "--changelog"]);
  assert.equal(result.code, 0, result.out);

  const changelog = readFileSync(join(root, "CHANGELOG.md"), "utf8");
  assert.match(changelog, /## \[0\.6\.0\] - \d{4}-\d{2}-\d{2}/);
  assert.match(changelog, /\[0\.6\.0\]: https:\/\/github\.com\/xinye83\/mscout\/releases\/tag\/v0\.6\.0/);
  assert.match(changelog, /\[Unreleased\]: https:\/\/github\.com\/xinye83\/mscout\/compare\/v0\.6\.0\.\.\.HEAD/);
  // The pending entry moved under the new section, leaving Unreleased empty.
  assert.match(changelog, /## \[0\.6\.0\] - [\d-]+\n\n### Added\n- A pending entry\./);
  assertUnchanged(root, before);

  // Idempotent: a second run adds nothing.
  const again = run(root, [VERSION, "--changelog"]);
  assert.equal(again.code, 0, again.out);
  assert.match(again.out, /already has a section for 0\.6\.0/);
  assert.equal(readFileSync(join(root, "CHANGELOG.md"), "utf8"), changelog);
});

test("--changelog with --dry-run leaves the changelog alone", (t) => {
  const root = tree(t);
  const before = snapshot(root);
  const result = run(root, ["0.7.0", "--changelog", "--dry-run"]);
  assert.equal(result.code, 0, result.out);
  assert.match(result.out, /promoted \[Unreleased\]/);
  assertUnchanged(root, before);
});

test("--check with an explicit target does not suggest a write", (t) => {
  const root = tree(t);
  const result = run(root, ["--check", "0.9.9"]);
  assert.equal(result.code, 1, result.out);
  assert.match(result.out, /6 of 6 version locations disagree with 0\.9\.9/);
  assert.doesNotMatch(result.out, /fix with: node scripts\/bump-version\.mjs 0\.9\.9/);
});

test("invalid invocations exit 2 with usage and touch nothing", (t) => {
  const root = tree(t);
  const before = snapshot(root);
  for (const args of [[], ["--nope"], ["v0.7.0"], ["0.7.0", "0.8.0"], ["--check", "--dry-run"]]) {
    const result = run(root, args);
    assert.equal(result.code, 2, `${args.join(" ")}: ${result.out}`);
    assert.match(result.out, /Usage: node scripts\/bump-version\.mjs/, args.join(" "));
  }
  assertUnchanged(root, before);
});
