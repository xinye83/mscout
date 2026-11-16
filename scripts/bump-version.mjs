#!/usr/bin/env node
// Version bump / drift check for MScout.
//
// Every stored version is read and written from one place. A write always needs
// an explicit version argument, so a bare run can never bump anything:
//
//   node scripts/bump-version.mjs --check            check all locations agree
//   node scripts/bump-version.mjs --check 0.7.0      check all locations are 0.7.0
//   node scripts/bump-version.mjs 0.7.0 --dry-run    show the edits, write nothing
//   node scripts/bump-version.mjs 0.7.0              write the edits
//   node scripts/bump-version.mjs 0.7.0 --changelog  ... and promote [Unreleased]
//
// Covered:
//   src-tauri/Cargo.toml       `version` inside the [package] table
//   src-tauri/Cargo.lock       `version` of the `mscout` [[package]] entry
//   package.json               top-level `version`
//   package-lock.json          top-level `version` and `packages[""].version`
//   src-tauri/tauri.conf.json  `version`
//
// The CHANGELOG is only written when --changelog is passed as well; with
// --check the same flag merely verifies that a released section exists.
// Nothing is ever promoted implicitly.
//
// Edits are textual and in place: no lockfile is regenerated and no build tool
// is invoked. Every edit is re-read and verified before it is written, and
// JSON files are re-parsed and deep-compared against the original so a bad
// match can never corrupt a file.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const CHANGELOG = "CHANGELOG.md";

class UsageError extends Error {}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

function eolOf(text) {
  return text.includes("\r\n") ? "\r\n" : "\n";
}

function lineOf(text, index) {
  return text.slice(0, index).split(/\r?\n/).length - 1;
}

// Offset of the first character of each line, for turning a line match into a
// text offset.
function lineStarts(lines, eol) {
  const starts = [];
  let offset = 0;
  for (const line of lines) {
    starts.push(offset);
    offset += line.length + eol.length;
  }
  return starts;
}

// A single version location inside one file:
//
//   { label, value, line, start, end }
//
// `start`..`end` is the range of the version text (the only thing an edit
// changes) and `line` is the 0-based index of the line holding it, which is how
// a write is proven to have touched nothing else.
//
// Entries carry offsets rather than a whole-text rewrite because one file can
// hold more than one of them (package-lock.json): the edits are spliced
// right-to-left so earlier offsets stay valid.
function applyEntries(text, entries, next) {
  return entries
    .slice()
    .sort((a, b) => b.start - a.start)
    .reduce((current, entry) => current.slice(0, entry.start) + next + current.slice(entry.end), text);
}

// The JSON locators stay plain regexes; the parse in verifyJson double-checks
// that they hit the intended key.
function jsonLocator(text, where, pattern, path) {
  const match = pattern.exec(text);
  if (!match) throw new Error(`could not find the ${where} version`);
  const [, prefix, value] = match;
  const start = match.index + prefix.length;
  return {
    label: where,
    path,
    value,
    // The line the value sits on, which is not the line the match starts on for
    // the multi-line workspace pattern.
    line: lineOf(text, start),
    start,
    end: start + value.length,
  };
}

function cargoManifestEntries(text) {
  const eol = eolOf(text);
  const lines = text.split(eol);
  const starts = lineStarts(lines, eol);
  const header = lines.findIndex((line) => line.trim() === "[package]");
  if (header === -1) throw new Error("no [package] table");

  let end = lines.length;
  for (let i = header + 1; i < lines.length; i += 1) {
    if (/^\s*\[/.test(lines[i])) {
      end = i;
      break;
    }
  }

  const hits = [];
  for (let i = header + 1; i < end; i += 1) {
    const match = /^(\s*version\s*=\s*")([^"]*)(".*)$/.exec(lines[i]);
    if (match) hits.push({ i, match });
  }
  if (hits.length !== 1) {
    throw new Error(`expected exactly one version in [package], found ${hits.length}`);
  }

  const { i, match } = hits[0];
  const start = starts[i] + match[1].length;
  return [
    {
      label: "[package] version",
      value: match[2],
      line: i,
      start,
      end: start + match[2].length,
    },
  ];
}

function cargoLockEntries(text) {
  const eol = eolOf(text);
  const lines = text.split(eol);
  const starts = lineStarts(lines, eol);

  // [[package]] blocks; anything at column 0 that is not a key belongs to no block.
  const blocks = [];
  let current = null;
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    if (line.trim() === "[[package]]") {
      current = { fields: [], start: i };
      blocks.push(current);
      continue;
    }
    if (/^\s*\[/.test(line)) {
      current = null;
      continue;
    }
    if (current && /^[A-Za-z_]/.test(line)) {
      const match = /^(\s*)([A-Za-z_][\w-]*)\s*=\s*(.*)$/.exec(line);
      if (match) current.fields.push({ i, key: match[2], line });
    }
  }

  const candidates = blocks.filter((block) => {
    const name = block.fields.find((field) => field.key === "name");
    return name && /^"mscout"\s*$/.test(name.line.slice(name.line.indexOf("=") + 1).trim());
  });
  if (candidates.length !== 1) {
    throw new Error(`expected exactly one mscout [[package]] entry, found ${candidates.length}`);
  }

  const version = candidates[0].fields.find((field) => field.key === "version");
  if (!version) throw new Error("the mscout [[package]] entry has no version");
  const match = /^(\s*version\s*=\s*")([^"]*)(".*)$/.exec(version.line);
  if (!match) throw new Error("the mscout version is not a plain string");

  const start = starts[version.i] + match[1].length;
  return [
    {
      label: "mscout package version",
      value: match[2],
      line: version.i,
      start,
      end: start + match[2].length,
    },
  ];
}

function packageJsonEntries(text) {
  return [
    jsonLocator(text, "package version", /^(\s{2}"version"\s*:\s*")([^"]*)(")/m, ["version"]),
  ];
}

function packageLockEntries(text) {
  return [
    jsonLocator(text, "lockfile version", /^(\s{2}"version"\s*:\s*")([^"]*)(")/m, ["version"]),
    jsonLocator(
      text,
      "workspace package version",
      /("":\s*\{\r?\n\s*"name":\s*"mscout",\r?\n\s*"version":\s*")([^"]*)(")/,
      ["packages", "", "version"],
    ),
  ];
}

function tauriConfigEntries(text) {
  return [
    jsonLocator(text, "app version", /^(\s{2}"version"\s*:\s*")([^"]*)(")/m, ["version"]),
  ];
}

const FILES = [
  { file: "src-tauri/Cargo.toml", locate: cargoManifestEntries },
  { file: "src-tauri/Cargo.lock", locate: cargoLockEntries },
  { file: "package.json", locate: packageJsonEntries },
  { file: "package-lock.json", locate: packageLockEntries },
  { file: "src-tauri/tauri.conf.json", locate: tauriConfigEntries },
];

// ---------------------------------------------------------------------------
// Reading and verification
// ---------------------------------------------------------------------------

function readFile(file) {
  try {
    return readFileSync(join(root, file), "utf8");
  } catch (error) {
    throw new Error(`cannot read ${file} (${error.code ?? error.message})`);
  }
}

function collect() {
  const entries = [];
  for (const target of FILES) {
    const text = readFile(target.file);
    for (const entry of target.locate(text)) entries.push({ file: target.file, ...entry });
  }
  return entries;
}

function locateIn(file, text) {
  const target = FILES.find((candidate) => candidate.file === file);
  if (!target) throw new Error(`no locator for ${file}`);
  return target.locate(text);
}

// Every changed line must be one of the lines we meant to change: same line
// count, and the only differing indices are the version lines.
function verifyLines(file, before, after, expectedLines) {
  const eol = eolOf(before);
  if (eol !== eolOf(after)) throw new Error(`${file}: line endings changed`);
  const oldLines = before.split(eol);
  const newLines = after.split(eol);
  if (oldLines.length !== newLines.length) throw new Error(`${file}: line count changed`);

  const allowed = new Set(expectedLines);
  for (let i = 0; i < oldLines.length; i += 1) {
    if (oldLines[i] !== newLines[i] && !allowed.has(i)) {
      throw new Error(`${file}: unexpected edit on line ${i + 1}`);
    }
  }
}

function setPath(object, path, value) {
  let cursor = object;
  for (const key of path.slice(0, -1)) cursor = cursor[key];
  cursor[path[path.length - 1]] = value;
}

// The edited document must differ from the original only where we meant it to:
// parse both, apply the intended paths to a copy of the original, compare.
function verifyJson(file, before, after, located, next) {
  const expected = JSON.parse(before);
  for (const entry of located) setPath(expected, entry.path, next);
  const actual = JSON.parse(after);
  if (JSON.stringify(expected) !== JSON.stringify(actual)) {
    throw new Error(`${file}: edit did not match the parsed document; refusing to write`);
  }
}

function verify(file, before, after, located, changed, next) {
  // Re-locate in the new text: the locators must still find the same number of
  // entries, now all holding the new value.
  const relaid = locateIn(file, after);
  if (relaid.length !== located.length) throw new Error(`${file}: version entry count changed`);
  for (const entry of relaid) {
    if (entry.value !== next) {
      throw new Error(`${file}: ${entry.label} reads ${entry.value} after the edit`);
    }
  }
  verifyLines(file, before, after, changed.map((entry) => entry.line));
  if (file.endsWith(".json")) verifyJson(file, before, after, located, next);
}

// ---------------------------------------------------------------------------
// Mode: check
// ---------------------------------------------------------------------------

function canonicalOf(entries) {
  const counts = new Map();
  for (const entry of entries) counts.set(entry.value, (counts.get(entry.value) ?? 0) + 1);
  const highest = Math.max(...counts.values());
  const tied = [...counts].filter(([, count]) => count === highest).map(([value]) => value);
  if (tied.length === 1) return tied[0];
  // A tie means the files disagree; prefer package.json, the version npm reads.
  const pkg = entries.find((entry) => entry.file === "package.json");
  return pkg && tied.includes(pkg.value) ? pkg.value : tied[0];
}

function runCheck(target) {
  const entries = collect();
  const expected = target ?? canonicalOf(entries);
  const source = target ? "from the argument" : "from the files";
  console.log(`MScout version check — expected ${expected} (${source})`);

  let drift = 0;
  for (const entry of entries) {
    const ok = entry.value === expected;
    if (!ok) drift += 1;
    const status = ok ? "ok   " : "drift";
    const found = ok ? entry.value : `found ${entry.value}, expected ${expected}`;
    console.log(`  ${status} ${entry.file.padEnd(26)} ${entry.label.padEnd(26)} ${found}`);
  }

  if (drift === 0) {
    console.log(`${entries.length} version locations agree on ${expected}`);
    return 0;
  }
  console.error(
    `${drift} of ${entries.length} version locations disagree with ${expected}; ` +
      `fix with: node scripts/bump-version.mjs ${expected}`,
  );
  return 1;
}

// ---------------------------------------------------------------------------
// Mode: write
// ---------------------------------------------------------------------------

function compareCore(a, b) {
  const pa = a.split(/[-+]/)[0].split(".").map(Number);
  const pb = b.split(/[-+]/)[0].split(".").map(Number);
  for (let i = 0; i < 3; i += 1) {
    if (pa[i] !== pb[i]) return pa[i] - pb[i];
  }
  return 0;
}

function today() {
  const now = new Date();
  const pad = (value) => String(value).padStart(2, "0");
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
}

// Promote `## [Unreleased]` to a released section for `next` and move the
// Unreleased compare link forward. Returns the new text, or a reason it was left
// alone. Only ever called when --changelog is passed explicitly.
function promoteChangelog(text, next) {
  const eol = eolOf(text);
  let lines = text.split(eol);

  const unreleased = lines.findIndex((line) => /^##\s+\[Unreleased\]\s*$/.test(line));
  if (unreleased === -1) return { text, promoted: false, reason: "no `## [Unreleased]` heading" };
  if (lines.some((line) => line.startsWith(`## [${next}]`))) {
    return { text, promoted: false, reason: `the changelog already has a section for ${next}` };
  }

  let nextHeading = lines.length;
  for (let i = unreleased + 1; i < lines.length; i += 1) {
    if (/^##\s+\[/.test(lines[i])) {
      nextHeading = i;
      break;
    }
  }
  const body = lines.slice(unreleased + 1, nextHeading);
  if (!body.some((line) => /^\s*[-*]\s+\S/.test(line))) {
    return { text, promoted: false, reason: "the Unreleased section is empty" };
  }

  lines.splice(unreleased + 1, 0, "", `## [${next}] - ${today()}`);

  const linkIndex = lines.findIndex((line) => /^\[Unreleased\]:\s*\S+/.test(line));
  if (linkIndex === -1) {
    return { text: lines.join(eol), promoted: true, reason: "no compare link to update" };
  }
  const link = /^(\[Unreleased\]:\s*)(\S*?\/compare\/)v?\S*?\.\.\.HEAD\s*$/.exec(lines[linkIndex]);
  if (!link) {
    return { text: lines.join(eol), promoted: true, reason: "compare link not rewritten" };
  }
  const base = link[2].replace(/compare\/$/, "");
  lines[linkIndex] = `${link[1]}${link[2]}v${next}...HEAD`;
  lines.splice(linkIndex + 1, 0, `[${next}]: ${base}releases/tag/v${next}`);

  return { text: lines.join(eol), promoted: true, reason: null };
}

function runWrite(next, options) {
  const entries = collect();
  const current = canonicalOf(entries);

  // Only the locations that actually disagree are rewritten, so the same
  // command both bumps a release and repairs a single drifted file.
  const drifted = entries.filter((entry) => entry.value !== next);
  if (drifted.length === 0) {
    console.log(`every file is already at ${next}; nothing to write`);
    return 0;
  }
  if (compareCore(next, current) < 0 && !options.force) {
    console.error(
      `refusing to move backwards: ${current} -> ${next}. Re-run with --force if that is intended.`,
    );
    return 1;
  }

  console.log(`${options.dryRun ? "dry run: " : ""}setting version ${next} (currently ${current})`);

  for (const target of FILES) {
    const before = readFile(target.file);
    const located = target.locate(before);
    const changed = located.filter((entry) => entry.value !== next);
    if (changed.length === 0) continue;

    const after = applyEntries(before, located, next);
    verify(target.file, before, after, located, changed, next);

    for (const entry of changed) {
      console.log(`  ${target.file.padEnd(26)} ${entry.label.padEnd(26)} ${entry.value} -> ${next}`);
    }
    if (!options.dryRun) writeFileSync(join(root, target.file), after);
  }

  if (!options.changelog) {
    console.log(`CHANGELOG.md left alone; add --changelog to promote [Unreleased] to ${next}`);
  } else {
    const before = readFile(CHANGELOG);
    const result = promoteChangelog(before, next);
    if (!result.promoted) {
      console.log(`CHANGELOG.md not promoted: ${result.reason}`);
    } else {
      if (!options.dryRun) writeFileSync(join(root, CHANGELOG), result.text);
      console.log(`  ${CHANGELOG.padEnd(26)} ${"promoted [Unreleased]".padEnd(26)} -> ${next}`);
      if (result.reason) console.log(`  note: ${result.reason}`);
    }
  }

  if (options.dryRun) {
    console.log("dry run: no files were written");
  } else {
    console.log("next: review `git diff`, then run `node scripts/bump-version.mjs --check`");
  }
  return 0;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

function usage() {
  return `Usage: node scripts/bump-version.mjs [--check] [options] [version]

Reads and writes every stored MScout version from one place.

Modes:
  --check               verify the stored versions and exit non-zero on drift.
                        Accepts an optional version to check against; without
                        one the majority value (package.json breaks ties) is
                        the expectation.
  <version>             write <version> into every location. A version is
                        mandatory for writes, so a bare run cannot bump.

Options:
  --dry-run             with a version: print the edits, write nothing
  --force               allow writing a version lower than the current one
  --changelog           write mode: promote CHANGELOG [Unreleased] to the new
                        version with today's date; check mode: require the
                        changelog to have a section and link for the version
  -h, --help            show this text

Files: ${FILES.map((target) => target.file).join(", ")}`;
}

function parseArgs(argv) {
  const options = { check: false, dryRun: false, force: false, changelog: false, help: false, version: null };
  for (const arg of argv) {
    if (arg === "--check") options.check = true;
    else if (arg === "--dry-run") options.dryRun = true;
    else if (arg === "--force") options.force = true;
    else if (arg === "--changelog") options.changelog = true;
    else if (arg === "-h" || arg === "--help") options.help = true;
    else if (arg.startsWith("-")) throw new UsageError(`unknown option ${arg}`);
    else if (options.version !== null) throw new UsageError(`more than one version argument (${options.version}, ${arg})`);
    else options.version = arg;
  }
  if (options.help) return options;
  if (options.version !== null && !SEMVER.test(options.version)) {
    throw new UsageError(`not a semantic version: ${options.version}`);
  }
  if (options.check && options.dryRun) throw new UsageError("--dry-run goes with a write, not --check");
  if (options.check && options.force) throw new UsageError("--force goes with a write, not --check");
  if (options.version === null && !options.check) {
    throw new UsageError("no version given; nothing was changed. Pass a version to write or --check to verify.");
  }
  return options;
}

function checkChangelog(version) {
  const text = readFile(CHANGELOG);
  const heading = text.split(/\r?\n/).some((line) => line.trim().startsWith(`## [${version}]`));
  const link = new RegExp(`^\\[${version.replace(/\./g, "\\.")}\\]:`, "m").test(text);
  if (heading && link) {
    console.log(`  ok    ${CHANGELOG.padEnd(26)} ${"released section".padEnd(26)} ${version}`);
    return 0;
  }
  const missing = [!heading && "section", !link && "link"].filter(Boolean).join(" and ");
  console.error(`  drift ${CHANGELOG.padEnd(26)} missing ${missing} for ${version}`);
  return 1;
}

function main(argv) {
  const options = parseArgs(argv);
  if (options.help) {
    console.log(usage());
    return 0;
  }

  if (!options.check) return runWrite(options.version, options);

  const code = runCheck(options.version);
  if (!options.changelog) return code;

  const expected = options.version ?? canonicalOf(collect());
  console.log("CHANGELOG check");
  const changelogCode = checkChangelog(expected);
  if (changelogCode !== 0) {
    console.error(`the changelog has no released section for ${expected}`);
  }
  return code === 0 ? changelogCode : code;
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  if (error instanceof UsageError) {
    console.error(`${error.message}\n`);
    console.error(usage());
    process.exitCode = 2;
  } else {
    console.error(`error: ${error.message}`);
    process.exitCode = 1;
  }
}
