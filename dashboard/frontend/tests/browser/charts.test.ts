import assert from "node:assert/strict";
import test from "node:test";
import { expect } from "@playwright/test";
import { browserView, fixturePage, seconds } from "./harness.ts";

type Gpu = "available" | "not_applicable" | "unavailable" | null;
type SeenStream = { url: string; closed: boolean };

function sample(time: number, mode: Gpu, percent: number | null, index = 0) {
  return {
    status: "available",
    checked: time,
    expires: time + 180,
    cpu_percent: index % 2 ? 2 : 4,
    memory_used: index % 2 ? 79 : 81,
    memory_total: 100,
    disks: [{ name: "root", used: 22, total: 100 }],
    ...(mode ? { gpu: { status: mode, percent } } : {}),
  };
}

test("telemetry draws useful trends and only streams while looking at machines", { timeout: 120_000 }, async () => {
  const now = seconds();
  const history = Array.from({ length: 30 }, (_, i) => sample(now - 590 + i * 20, "available", i % 2 ? 80 : 5, i));
  const cases: [Gpu, number | null][] = [["available", 0], ["available", 25], ["not_applicable", null], ["unavailable", null], [null, null]];
  const machines = cases.map(([mode, percent], i) => ({
    id: `fixture-${i}`, name: `Test host ${i}`, host: `test-${i}`,
    kind: "machine", lifecycle: "active", capabilities: [],
    metrics: { ...sample(now, mode, percent), history },
  }));
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
      const samplePanel = panels.nth(1);
      for (const [name, range, varied] of [["cpu", "0–10%", true], ["ram", "75–85%", true], ["disk", "0–100%", false]] as const) {
        const values = await samplePanel.locator(`.${name} .history-bar`).evaluateAll(bars => bars.map(bar => Number(bar.getAttribute("height"))));
        assert.equal(values.length, 30);
        const spread = Math.max(...values) - Math.min(...values);
        if (varied) assert.ok(spread > 5, name);
        else assert.equal(spread, 0, name);
        await expect(samplePanel.locator(`.${name} [data-range]`)).toHaveAttribute("data-range", range);
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
