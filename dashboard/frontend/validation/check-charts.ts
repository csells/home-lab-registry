import { readFile } from "node:fs/promises";
import { chromium } from "@playwright/test";
import { assertChartRanges, telemetryFixtures } from "./telemetry.ts";

const [target, extra] = process.argv.slice(2);
const login = process.env.LAB_LOGIN_JSON;
if (!target || extra || !login) {
  throw new Error("Supply an HTTPS disposable preview URL and a private LAB_LOGIN_JSON file.");
}
const review = new URL(target);
if (review.protocol !== "https:" || review.username || review.password) {
  throw new Error("Use an HTTPS preview URL without embedded credentials.");
}
const { username, password } = JSON.parse(await readFile(login, "utf8"));
if (typeof username !== "string" || !username || typeof password !== "string" || !password) {
  throw new Error("LAB_LOGIN_JSON must contain the preview username and password.");
}
const browser = await chromium.launch({ executablePath: process.env.LAB_CHROME_EXECUTABLE });
try {
  const page = await browser.newPage({
    httpCredentials: { username, password, origin: review.origin },
    viewport: { width: 393, height: 900 },
  });
  const machinePage = new URL("#machines", review);
  const response = await page.goto(machinePage.href, { waitUntil: "domcontentloaded" });
  if (!response?.ok() || new URL(page.url()).origin !== review.origin) {
    throw new Error("The disposable dashboard did not accept the preview login.");
  }
  await page.locator("#machines-tab").waitFor();
  const now = Date.now() / 1000;
  const machines = telemetryFixtures(now);
  const inventory = new URL("api/inventory", page.url());
  await page.route(url => url.origin === review.origin && url.pathname === inventory.pathname,
    route => route.fulfill({ json: { machines, services: [], backups: [], fetched_at: now, server_time: now } }));
  await page.route(url => url.origin === review.origin && url.pathname.endsWith("/stream"), route => route.abort());
  await page.reload();
  const panel = page.locator(".metrics-panel").nth(1);
  await panel.waitFor();
  await assertChartRanges(panel);
  process.stdout.write("PASS authenticated preview rendered 30 changing CPU/RAM bars and constant disk bars on explicit scales from fixture inputs.\n");
} finally {
  await browser.close();
}
