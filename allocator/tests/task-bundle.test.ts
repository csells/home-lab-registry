import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import assert from "node:assert/strict";
import test from "node:test";

const pack = new URL("../package-task-clients.ts", import.meta.url);
const platforms = ["linux-x86_64", "macos-arm64", "windows-x86_64"];

test("the native task bundle includes both recovery helpers beside every client", async () => {
  const root = await mkdtemp(join(tmpdir(), "lease-bundle-"));
  try {
    const dest = join(root, "out");
    for (const platform of platforms) {
      const path = join(root, platform);
      await mkdir(path);
      const client = join(path, platform.startsWith("windows") ? "lab-lease.exe" : "lab-lease");
      await writeFile(client, platform);
      await chmod(client, 0o700);
    }
    await writeFile(join(root, "linux-x86_64/lab-restore-workspace"), "linux", { mode: 0o700 });
    const args = [pack.pathname, root, dest];
    const withoutApple = spawnSync(process.execPath, args, { encoding: "utf8" });
    assert.notEqual(withoutApple.status, 0);
    assert.match(withoutApple.stderr, /macos-arm64\/lab-restore-workspace/);

    await writeFile(join(root, "macos-arm64/lab-restore-workspace"), "apple", { mode: 0o700 });
    execFileSync(process.execPath, args);
    for (const platform of platforms) {
      const base = join(dest, platform);
      assert.equal(await readFile(join(base, "restore-helpers/debian13-amd64/lab-restore-workspace"), "utf8"), "linux");
      assert.equal(await readFile(join(base, "restore-helpers/macos-xcode-arm64/lab-restore-workspace"), "utf8"), "apple");
      assert.match(await readFile(join(base, "SHA256SUMS"), "utf8"), /[0-9a-f]{64}  restore-helpers\/debian13-amd64\/lab-restore-workspace/);
    }
  } finally {
    await rm(root, { force: true, recursive: true });
  }
});
