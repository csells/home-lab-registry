import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { chromium, type Browser, type Page } from "@playwright/test";

const example = new URL("../../../rust/target/debug/examples/browser_fixture", import.meta.url);
const binary = process.env.LAB_BROWSER_FIXTURE ?? fileURLToPath(example);

export function seconds(): number {
  return Date.now() / 1000;
}

export async function browserView(
  run: (browser: Browser, origin: string) => Promise<void>,
  streamRows?: Record<string, unknown>,
): Promise<void> {
  const directory = streamRows && await mkdtemp(join(tmpdir(), "lab-browser-"));
  const input = directory && join(directory, "stream.json");
  if (input) await writeFile(input, JSON.stringify(streamRows), { mode: 0o600 });
  const server = spawn(binary, input ? [input] : [], { stdio: ["ignore", "pipe", "inherit"] });
  const lines = createInterface({ input: server.stdout });
  let browser: Browser | undefined;
  let timer: NodeJS.Timeout | undefined;
  try {
    const origin = await Promise.race([
      once(lines, "line").then(([line]) => String(line)),
      once(server, "exit").then(([code]) => {
        throw new Error("Dashboard fixture exited with " + code);
      }),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("Dashboard fixture did not start")), 5000);
      }),
    ]);
    if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(origin)) throw new Error("Invalid loopback origin");
    const executablePath = process.env.DASHBOARD_CHROMIUM ?? process.env.REVIEW_CHROMIUM;
    browser = await chromium.launch({ executablePath, headless: true });
    await run(browser, origin);
  } finally {
    if (timer) clearTimeout(timer);
    lines.close();
    await browser?.close();
    if (server.exitCode === null && server.pid) {
      server.kill();
      await once(server, "exit");
    }
    if (directory) await rm(directory, { recursive: true, force: true });
  }
}

export async function fixturePage(browser: Browser, width: number, height = 900): Promise<Page> {
  const page = await browser.newPage({
    viewport: { width, height },
    httpCredentials: { username: "fixture", password: "fixture" },
  });
  await page.route("**/api/inventory/stream", route => route.abort());
  return page;
}
