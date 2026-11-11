import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, onTestFinished, test } from "vitest";
import { siteRoot } from "../support/site";

// public/install.sh is served at https://nouli.ai/install.sh. These tests run it against
// stand-in `uname`, `getconf`, and `curl` commands and a local release archive.
const installer = join(siteRoot, "public/install.sh");
const version = "9.8.7";

interface FixtureOptions {
  system?: string;
  arch?: string;
  libc?: string;
  badBinary?: boolean;
  env?: Record<string, string>;
}

/** A stand-in `curl` that records each URL and serves the latest tag, checksums, and the archive. */
const fakeCurl = `#!${process.execPath}
const fs = require('node:fs');
const args = process.argv.slice(2);
const url = args.find(a => a.startsWith('https://'));
fs.appendFileSync(process.env.FAKE_REQUESTS, url + '\\n');
if (url.endsWith('/latest')) {
  process.stdout.write('https://github.com/nouli-ai/nouli/releases/tag/v${version}');
} else {
  if (process.env.FAKE_DOWNLOAD_FAILURE === '1') process.exit(22);
  const output = args[args.indexOf('-o') + 1];
  if (url.endsWith('.sha256')) {
    const name = url.split('/').at(-1).slice(0, -7);
    fs.writeFileSync(output, process.env.FAKE_CHECKSUM + '  ' + name + '\\n');
  } else {
    fs.copyFileSync(process.env.FAKE_ARCHIVE, output);
  }
}
`;

function fixture(options: FixtureOptions = {}) {
  const root = mkdtempSync(join(tmpdir(), "nouli-installer-test-"));
  onTestFinished(() => {
    rmSync(root, { recursive: true, force: true });
  });
  const bin = join(root, "bin");
  const work = join(root, "work");
  const dest = join(root, "destination with spaces");
  for (const dir of [bin, work, dest]) mkdirSync(dir);
  const executable = (name: string, contents: string): void => {
    writeFileSync(join(bin, name), contents, { mode: 0o755 });
  };
  executable(
    "uname",
    '#!/bin/sh\ncase "$1" in -s) printf "%s\\n" "$FAKE_SYSTEM";; -m) printf "%s\\n" "$FAKE_ARCH";; esac\n',
  );
  executable("getconf", '#!/bin/sh\nprintf "%s\\n" "$FAKE_LIBC"\n');
  executable("curl", fakeCurl);
  const body =
    options.badBinary === true
      ? "#!/bin/sh\nprintf 'nouli wrong-version\\n'\n"
      : `#!/bin/sh\nprintf 'nouli ${version}\\n'\n`;
  writeFileSync(join(work, "nouli"), body, { mode: 0o755 });
  const archive = join(root, "archive.tar.gz");
  const packed = spawnSync("tar", ["-czf", archive, "-C", work, "nouli"], { encoding: "utf8" });
  expect(packed.status, packed.stderr).toBe(0);
  const requests = join(root, "requests");
  const env = {
    ...process.env,
    PATH: `${bin}:${process.env["PATH"] ?? ""}`,
    TMPDIR: work,
    NOULI_INSTALL_DIR: dest,
    NOULI_VERSION: "latest",
    FAKE_SYSTEM: options.system ?? "Darwin",
    FAKE_ARCH: options.arch ?? "arm64",
    FAKE_LIBC: options.libc ?? "glibc 2.39",
    FAKE_CHECKSUM: createHash("sha256").update(readFileSync(archive)).digest("hex"),
    FAKE_ARCHIVE: archive,
    FAKE_REQUESTS: requests,
    ...options.env,
  };
  const run = (piped = false) =>
    spawnSync("sh", piped ? [] : [installer], {
      env,
      encoding: "utf8",
      timeout: 10_000,
      ...(piped ? { input: readFileSync(installer) } : {}),
    });
  return { run, dest, requests, body };
}

const requested = (requests: string): string => readFileSync(requests, "utf8");
const installed = (dest: string): string => readFileSync(join(dest, "nouli"), "utf8");

test.each([
  ["Darwin", "arm64", "aarch64-apple-darwin"],
  ["Linux", "aarch64", "aarch64-unknown-linux-gnu"],
  ["Linux", "x86_64", "x86_64-unknown-linux-gnu"],
])("installs the matching %s/%s release through sh stdin", (system, arch, target) => {
  const f = fixture({ system, arch });
  const result = f.run(true);
  expect(result.status, result.stderr).toBe(0);
  expect(installed(f.dest)).toBe(f.body);
  expect(requested(f.requests)).toContain(target);
  expect(readdirSync(f.dest)).toHaveLength(1);
});

test("a pinned version skips latest-release discovery", () => {
  const f = fixture({ env: { NOULI_VERSION: `v${version}` } });
  expect(f.run().status).toBe(0);
  expect(requested(f.requests)).not.toMatch(/\/latest/u);
});

test("checksum failure preserves the previous executable", () => {
  const f = fixture({ env: { FAKE_CHECKSUM: "0".repeat(64) } });
  writeFileSync(join(f.dest, "nouli"), "old executable");
  const result = f.run();
  expect(result.status).not.toBe(0);
  expect(result.stderr).toMatch(/checksum mismatch/u);
  expect(installed(f.dest)).toBe("old executable");
});

test("a wrong executable version preserves the previous installation", () => {
  const f = fixture({ badBinary: true });
  writeFileSync(join(f.dest, "nouli"), "old executable");
  expect(f.run().status).not.toBe(0);
  expect(installed(f.dest)).toBe("old executable");
  expect(readdirSync(f.dest)).toEqual(["nouli"]);
});

test.each(["glibc 2.37", "musl 1.2.5", ""])(
  "rejects incompatible Linux libc '%s' before downloading",
  (libc) => {
    const f = fixture({ system: "Linux", arch: "x86_64", libc });
    expect(f.run().status).not.toBe(0);
    expect(existsSync(f.requests)).toBe(false);
  },
);

test("accepts the supported glibc 2.38 boundary", () => {
  const f = fixture({ system: "Linux", arch: "aarch64", libc: "glibc 2.38" });
  expect(f.run().status).toBe(0);
});

test.each([
  ["Darwin", "x86_64"],
  ["Linux", "armv7l"],
  ["Windows_NT", "AMD64"],
])("rejects unsupported %s/%s before downloading", (system, arch) => {
  const f = fixture({ system, arch });
  expect(f.run().status).not.toBe(0);
  expect(existsSync(f.requests)).toBe(false);
});

test("download failure leaves an existing executable untouched", () => {
  const f = fixture({ env: { FAKE_DOWNLOAD_FAILURE: "1" } });
  writeFileSync(join(f.dest, "nouli"), "old executable");
  expect(f.run().status).not.toBe(0);
  expect(installed(f.dest)).toBe("old executable");
});

test("rejects an invalid pinned version before downloading", () => {
  const f = fixture({ env: { NOULI_VERSION: "../../other" } });
  expect(f.run().status).not.toBe(0);
  expect(existsSync(f.requests)).toBe(false);
});
