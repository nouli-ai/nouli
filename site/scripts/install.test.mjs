import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, readdirSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

const installer = resolve(import.meta.dirname, "../public/install.sh");
const version = "9.8.7";

function fixture(t, options = {}) {
  const root = mkdtempSync(join(tmpdir(), "nouli-installer-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, "bin"), work = join(root, "work"), dest = join(root, "destination with spaces");
  mkdirSync(bin); mkdirSync(work); mkdirSync(dest);
  const executable = (name, contents) => writeFileSync(join(bin, name), contents, { mode: 0o755 });
  executable("uname", '#!/bin/sh\ncase "$1" in -s) printf "%s\\n" "$FAKE_SYSTEM";; -m) printf "%s\\n" "$FAKE_ARCH";; esac\n');
  executable("getconf", '#!/bin/sh\nprintf "%s\\n" "$FAKE_LIBC"\n');
  const body = options.badBinary ? "#!/bin/sh\nprintf 'nouli wrong-version\\n'\n" : `#!/bin/sh\nprintf 'nouli ${version}\\n'\n`;
  writeFileSync(join(work, "nouli"), body, { mode: 0o755 });
  const archive = join(root, "archive.tar.gz");
  const packed = spawnSync("tar", ["-czf", archive, "-C", work, "nouli"]);
  assert.equal(packed.status, 0, packed.stderr.toString());
  executable("curl", `#!${process.execPath}
const fs = require('node:fs');
const args = process.argv.slice(2);
const url = args.find(a => a.startsWith('https://'));
fs.appendFileSync(process.env.FAKE_REQUESTS, url + '\\n');
if (url.endsWith('/latest')) {
  process.stdout.write('https://github.com/standing-desks/nouli/releases/tag/v${version}');
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
`);
  const checksum = createHash("sha256").update(readFileSync(archive)).digest("hex");
  const requests = join(root, "requests");
  const env = {
    ...process.env, PATH: bin + ":" + process.env.PATH,
    TMPDIR: work, NOULI_INSTALL_DIR: dest, NOULI_VERSION: "latest",
    FAKE_SYSTEM: options.system ?? "Darwin", FAKE_ARCH: options.arch ?? "arm64",
    FAKE_LIBC: options.libc ?? "glibc 2.39", FAKE_CHECKSUM: checksum,
    FAKE_ARCHIVE: archive, FAKE_REQUESTS: requests,
    ...options.env,
  };
  const run = (piped = false) => spawnSync("sh", piped ? [] : [installer], {
    env, input: piped ? readFileSync(installer) : undefined, encoding: "utf8", timeout: 10000,
  });
  return { run, env, dest, requests, root, checksum, body };
}

for (const [system, arch, target] of [
  ["Darwin", "arm64", "aarch64-apple-darwin"],
  ["Linux", "aarch64", "aarch64-unknown-linux-gnu"],
  ["Linux", "x86_64", "x86_64-unknown-linux-gnu"],
]) {
  test(`installs the matching ${system}/${arch} release through sh stdin`, t => {
    const f = fixture(t, { system, arch });
    const result = f.run(true);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(readFileSync(join(f.dest, "nouli"), "utf8"), f.body);
    assert.match(readFileSync(f.requests, "utf8"), new RegExp(target));
    assert.equal(readdirSync(f.dest).length, 1);
  });
}

test("a pinned version skips latest-release discovery", t => {
  const f = fixture(t, { env: { NOULI_VERSION: "v" + version } });
  assert.equal(f.run().status, 0);
  assert.doesNotMatch(readFileSync(f.requests, "utf8"), /\/latest/);
});

test("checksum failure preserves the previous executable", t => {
  const f = fixture(t, { env: { FAKE_CHECKSUM: "0".repeat(64) } });
  writeFileSync(join(f.dest, "nouli"), "old executable");
  const result = f.run();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /checksum mismatch/);
  assert.equal(readFileSync(join(f.dest, "nouli"), "utf8"), "old executable");
});

test("a wrong executable version preserves the previous installation", t => {
  const f = fixture(t, { badBinary: true });
  writeFileSync(join(f.dest, "nouli"), "old executable");
  assert.notEqual(f.run().status, 0);
  assert.equal(readFileSync(join(f.dest, "nouli"), "utf8"), "old executable");
  assert.deepEqual(readdirSync(f.dest), ["nouli"]);
});

for (const libc of ["glibc 2.37", "musl 1.2.5", ""]) {
  test(`rejects incompatible Linux libc '${libc}' before downloading`, t => {
    const f = fixture(t, { system: "Linux", arch: "x86_64", libc });
    assert.notEqual(f.run().status, 0);
    assert.equal(existsSync(f.requests), false);
  });
}

test("accepts the supported glibc 2.38 boundary", t => {
  const f = fixture(t, { system: "Linux", arch: "aarch64", libc: "glibc 2.38" });
  assert.equal(f.run().status, 0);
});

for (const [system, arch] of [["Darwin", "x86_64"], ["Linux", "armv7l"], ["Windows_NT", "AMD64"]]) {
  test(`rejects unsupported ${system}/${arch} before downloading`, t => {
    const f = fixture(t, { system, arch });
    assert.notEqual(f.run().status, 0);
    assert.equal(existsSync(f.requests), false);
  });
}

test("download failure leaves an existing executable untouched", t => {
  const f = fixture(t, { env: { FAKE_DOWNLOAD_FAILURE: "1" } });
  writeFileSync(join(f.dest, "nouli"), "old executable");
  assert.notEqual(f.run().status, 0);
  assert.equal(readFileSync(join(f.dest, "nouli"), "utf8"), "old executable");
});

test("rejects an invalid pinned version before downloading", t => {
  const f = fixture(t, { env: { NOULI_VERSION: "../../other" } });
  assert.notEqual(f.run().status, 0);
  assert.equal(existsSync(f.requests), false);
});
