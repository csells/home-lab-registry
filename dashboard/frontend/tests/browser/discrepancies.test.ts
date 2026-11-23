import assert from "node:assert/strict";
import test from "node:test";
import { expect } from "@playwright/test";
import { browserView, fixturePage, seconds } from "./harness.ts";

test("discrepancies count failures and remember reconciliation and paginated lease state without SSE", { timeout: 120_000 }, async () => {
  await browserView(async (browser, origin) => {
    for (const width of [390, 1280]) {
      const page = await fixturePage(browser, width, 850);
      const errors: string[] = [];
      page.on("pageerror", error => errors.push(error.message));
      const stamp = seconds();
      const accuracy = {
        checked: stamp, hosts: [], findings: [], coverage: { verified: 2, expected: 2, unconfigured: 0 },
      };
      const snapshot = {
        fetched_at: stamp, server_time: stamp, machines: [], backups: [], lease_review_enabled: true,
        services: [{ id: "web", name: "Web", host: "test", lifecycle: "active", capabilities: [],
          health: { status: "unhealthy", expires: stamp + 60 } }],
        inventory_accuracy: accuracy,
      };
      await page.route("**/api/inventory", route => route.fulfill({ json: snapshot }));
      await page.route("**/api/reconcile", route => route.fulfill({ status: 202, json: { id: "owned" } }));
      await page.route("**/api/reconciliation", route => route.fulfill({
        json: { comparison: { id: "owned", state: "complete" }, inventory_accuracy: accuracy },
      }));
      await page.route("**/api/leases?*", route => {
        const params = new URL(route.request().url()).searchParams;
        const current = (params.get("scope") ?? "current") === "current";
        const more = params.has("before");
        const task = current ? "Current task" : more ? "Older task" : "Recent release";
        return route.fulfill({ json: {
          as_of: stamp, counts: { current: 1, released: 2 },
          leases: [{ id: task, task, owner: "fixture", phase: current ? "ready" : "released",
            deadline: stamp + 60, checkpoint_count: 0 }],
          next_before: current || more ? null : 2,
        } });
      });
      await page.goto(origin + "/#discrepancies");
      const badge = page.locator("#discrepancy-count");
      await expect(badge).toHaveText("1");
      await expect(badge).toHaveClass("has-errors");
      await expect(page.locator("#accuracy-findings")).toContainText("Web — Health check failed.");
      await page.getByRole("button", { name: "Reconcile now" }).click();
      await expect(page.locator("#reconcile-status")).toContainText("Comparison finished · 2 of 2", { timeout: 8000 });
      await page.locator("#services-tab").click();
      await expect(badge).toHaveText("1");
      await expect(page.locator("#cards")).toContainText("Web");
      await page.locator("#leases-tab").click();
      await expect(page.locator("#leases")).toContainText("Current task");
      await page.getByRole("button", { name: "History 2" }).click();
      await expect(page.locator("#leases")).toContainText("Recent release");
      await page.getByRole("button", { name: "Show more leases" }).click();
      await expect(page.locator("#leases")).toContainText("Older task");
      await expect(page.locator("#leases")).toContainText("Recent release");
      await expect(page.locator("#leases")).not.toContainText("Current task");
      assert.deepEqual(errors, []);
      await page.close();
    }
  });
});
