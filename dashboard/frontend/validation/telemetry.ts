import assert from "node:assert/strict";
import type { Locator } from "@playwright/test";

type Gpu = "available" | "not_applicable" | "unavailable" | null;

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

export function telemetryFixtures(now: number) {
  const history = Array.from({ length: 30 }, (_, i) => sample(now - 590 + i * 20, "available", i % 2 ? 80 : 5, i));
  const cases: [Gpu, number | null][] = [["available", 0], ["available", 25], ["not_applicable", null], ["unavailable", null], [null, null]];
  return cases.map(([mode, percent], i) => ({
    id: `fixture-${i}`, name: `Test host ${i}`, host: `test-${i}`,
    kind: "machine", lifecycle: "active", capabilities: [],
    metrics: { ...sample(now, mode, percent), history },
  }));
}

export async function assertChartRanges(panel: Locator) {
  for (const [name, range, varied] of [["cpu", "0–10%", true], ["ram", "75–85%", true], ["disk", "0–100%", false]] as const) {
    const cell = panel.locator(`.${name}`);
    const values = await cell.locator(".history-bar").evaluateAll(bars => bars.map(bar => Number(bar.getAttribute("height"))));
    assert.equal(values.length, 30, name);
    const spread = Math.max(...values) - Math.min(...values);
    if (varied) assert.ok(spread > 5, name);
    else assert.equal(spread, 0, name);
    assert.equal(await cell.locator("[data-range]").getAttribute("data-range"), range);
  }
}
