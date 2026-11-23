import { mkdir, stat } from "node:fs/promises";
import { resolve } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";
import { expect } from "@playwright/test";
import { browserView, fixturePage, seconds } from "./harness.ts";

import { assertChartRanges, telemetryFixtures } from "../../validation/telemetry.ts";

type SeenStream = { url: string; closed: boolean };

test("telemetry draws useful trends and only streams while looking at machines", { timeout: 120_000 }, async () => {
  const now = seconds();
  const machines = telemetryFixtures(now);
  await browserView(async (browser, origin) => {
    for (const width of [393, 1440]) {
      const page = await fixturePage(browser, width);
      await page.addInitScript(() => {
        const target = window as unknown as { metricStreams: SeenStream[] };
        target.metricStreams = [];
        const Native = window.EventSource;
        window.EventSource = class extends Native {
          signal?: SeenStream;
          constructor(url: string | URL, configuration?: EventSourceInit) {
            super(url, configuration);
            if (String(url).includes("/metrics/")) {
              this.signal = { url: String(url), closed: false };
              target.metricStreams.push(this.signal);
            }
          }
          close() {
            if (this.signal) this.signal.closed = true;
            super.close();
          }
        };
      });
      await page.route("**/api/inventory", route => route.fulfill({ json: { machines, services: [], backups: [], fetched_at: now } }));
      await page.route("**/api/metrics/stream?*", route => {
        const query = new URL(route.request().url()).search;
        return route.continue({ url: origin + "/__fixtures/metrics" + query });
      });
      await page.goto(origin);
      const panels = page.locator(".metrics-panel");
      await expect(panels).toHaveCount(5);
      await expect(panels.first().locator(".cpu .telemetry-value")).toHaveText("51.0%");
      const gpu = await panels.evaluateAll(elements => elements.map(panel => {
        const cell = panel.querySelector(".gpu")!;
        const heights = [...cell.querySelectorAll(".history-bar")].map(bar => Number(bar.getAttribute("height")));
        return {
          value: cell.querySelector(".telemetry-value")?.textContent,
          overlay: cell.querySelector(".telemetry-overlay")?.textContent ?? null,
          cells: panel.querySelectorAll(".telemetry-cell").length,
          spread: Math.max(...heights) - Math.min(...heights),
          fits: panel.scrollWidth <= panel.clientWidth,
        };
      }));
      assert.ok(gpu.every(result => result.cells === 4 && result.fits));
      assert.equal(gpu[0]!.value, "0.0%");
      assert.equal(gpu[0]!.overlay, null);
      assert.equal(gpu[1]!.value, "25.0%");
      assert.ok(gpu[1]!.spread > 20);
      assert.deepEqual(gpu.slice(2).map(value => value.overlay), ["N/A", "Unavailable", "Unavailable"]);
      await assertChartRanges(panels.nth(1));
      const output = process.env.LAB_GPU_SCREENSHOTS;
      if (output) {
        const file = resolve(output, `gpu-review-${width}.png`);
        await mkdir(output, { recursive: true });
        await page.screenshot({ path: file, fullPage: true });
        assert.ok((await stat(file)).size > 1024, "Rendered telemetry evidence must be captured");
      }
      await page.locator("#services-tab").click();
      await page.waitForFunction(() => {
        const streams = (window as unknown as { metricStreams: SeenStream[] }).metricStreams;
        return streams.length && streams.every(stream => stream.closed);
      });
      const before = await page.evaluate(() => (window as unknown as { metricStreams: SeenStream[] }).metricStreams.length);
      await page.locator("#machines-tab").click();
      await page.waitForFunction(past => (window as unknown as { metricStreams: SeenStream[] }).metricStreams.length > past, before);
      await page.close();
    }
  }, Object.fromEntries(machines.map(machine => [
    machine.id,
    machine.id === "fixture-0" ? { ...machine.metrics, cpu_percent: 51 } : machine.metrics,
  ])));
});
