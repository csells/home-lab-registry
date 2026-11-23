import { createHash } from "node:crypto";
import { chmod, copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

const [source, output, extra] = process.argv.slice(2);
if (!source || !output || extra) {
  throw new Error("Usage: node allocator/package-task-clients.ts NATIVE_BUILDS NEW_OUTPUT");
}

const platforms = ["linux-x86_64", "macos-arm64", "windows-x86_64"] as const;
const executable = (name: string, platform: string) => platform.startsWith("windows") ? name + ".exe" : name;
const programs = ["lab-lease", "lab-registry-client"];
const inputs = [
  ...platforms.flatMap(platform => programs.map(name => [platform, executable(name, platform)] as const)),
  ["linux-x86_64", "lab-restore-workspace"],
  ["macos-arm64", "lab-restore-workspace"],
] as const;
const hashes = new Map();
for (const [platform, name] of inputs) {
  const relative = `${platform}/${name}`;
  const bytes = await readFile(join(source, relative)).catch(() => {
    throw new Error("Missing native input " + relative);
  });
  if (!bytes.length) throw new Error("Empty native input " + relative);
  hashes.set(relative, createHash("sha256").update(bytes).digest("hex"));
}

await mkdir(output, { mode: 0o700 });
for (const platform of platforms) {
  const target = join(output, platform);
  const executables = new Map([
    ...programs.map(name => {
      const file = executable(name, platform);
      return [file, `${platform}/${file}`] as const;
    }),
    ["restore-helpers/debian13-amd64/lab-restore-workspace", "linux-x86_64/lab-restore-workspace"],
    ["restore-helpers/macos-xcode-arm64/lab-restore-workspace", "macos-arm64/lab-restore-workspace"],
  ]);
  const checksums: string[] = [];
  for (const [destination, input] of executables) {
    const path = join(target, destination);
    await mkdir(dirname(path), { recursive: true, mode: 0o700 });
    await copyFile(join(source, input), path);
    await chmod(path, 0o700);
    checksums.push(`${hashes.get(input)}  ${destination}\n`);
  }
  await writeFile(join(target, "SHA256SUMS"), checksums.join(""), { mode: 0o600 });
}
