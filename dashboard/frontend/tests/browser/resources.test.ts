import assert from "node:assert/strict";
import test from "node:test";
import { expect } from "@playwright/test";
import { browserView, fixturePage } from "./harness.ts";

function row(id: string, type = "machine", parent: string | null = null, host = "synthetic-host", dependencies: string[] = []) {
  return {
    id, name: id, display_name: id, host, kind: type === "service" ? "service" : "machine",
    machine_type: type, machine_id: parent, dependencies, capabilities: [], endpoints: {},
    health: { status: "unknown" }, metrics: { status: "unconfigured" },
  };
}

test("ELK containment and dependency edges render with the real static routes and CSP", { timeout: 120_000 }, async () => {
  const catalog = {
    machines: [
      { ...row("Linux"), metrics: { status: "waiting" } },
      row("Mac", "machine", "Linux", "synthetic-mac"),
      row("Container", "container", "Linux"),
      { ...row("VM", "virtual-machine", "Linux"), runtime: { state: "stopped", expires: 4102444800 } },
      row("Orphan", "container", "missing"),
      row("Cycle-a", "container", "Cycle-b"),
      row("Cycle-b", "container", "Cycle-a"),
    ],
    services: [
      row("Native app", "service", "Linux", "synthetic-host", ["Guest app"]),
      row("Guest app", "service", "Container", "synthetic-host", ["Missing backend"]),
      row("<img src=x onerror=alert(1)>", "service", "VM"),
    ],
    backups: [row("Backup job", "backup-job")],
  };
  await browserView(async (browser, origin) => {
    for (const width of [1440, 393]) {
      const page = await fixturePage(browser, width, 1000);
      const errors: string[] = [];
      const requests: string[] = [];
      page.on("pageerror", error => errors.push(error.message));
      page.on("request", request => requests.push(request.url()));
      await page.route("**/api/inventory", route => route.fulfill({ json: catalog }));
      await page.route("**/api/metrics/stream?*", route => route.fulfill({
        contentType: "text/event-stream", body: 'event: metrics\ndata: {"status":"waiting"}\n\n',
      }));
      await page.goto(origin + "/#map");
      await page.locator('#cards[data-layout-ready="true"]').waitFor();
      const nodes = page.locator(".resource-node[data-resource-id]");
      const edges = page.locator(".resource-edge");
      await expect(nodes).toHaveCount(10);
      await expect(edges).toHaveCount(1);
      await expect(page.locator("#cards button")).toHaveCount(0);
      const box = await page.locator(".graph-viewport svg").boundingBox();
      assert.ok(box && box.width > 200 && box.height > 200);
      await expect(page.locator(".graph-status")).toContainText("outside this view");
      await expect(page.locator("#cards img")).toHaveCount(0);
      await expect(page.locator("#cards")).not.toContainText("Backup job");
      await page.locator("#search").fill("Guest app");
      await expect(nodes).toHaveCount(3);
      await expect(edges).toHaveCount(0);
      await page.locator("#search").fill("");
      await page.locator("#host").selectOption("synthetic-mac");
      await expect(nodes).toHaveCount(1);
      await page.locator("#host").selectOption("");
      await page.locator('[data-filter="stopped"]').click();
      await expect(page.locator('.resource-node[data-resource-id="VM"]')).toHaveCount(1);
      await expect(page.locator('.resource-node[data-resource-id="Container"]')).toHaveCount(0);
      await page.locator('[data-filter="all"]').click();
      await expect(nodes).toHaveCount(10);
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
      assert.ok(!requests.some(request => request.endsWith("/app.js")));
      assert.deepEqual(errors, []);
      await page.locator("#machines-tab").click();
      await expect(page.locator("#cards .machine-card")).toHaveCount(7);
      await page.locator("#services-tab").click();
      await expect(page.locator("#cards .service-card")).toHaveCount(3);
      await page.locator("#backups-tab").click();
      await expect(page.locator("#cards .service-card")).toHaveCount(1);
      await page.close();
    }
  });
});
