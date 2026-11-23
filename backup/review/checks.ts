const statusElement = document.querySelector<HTMLElement>("#run-status");

async function refreshStatus(): Promise<void> {
  if (!statusElement) return;
  try {
    const response = await fetch("/report.json", { cache: "no-store" });
    if (!response.ok) throw new Error("Report unavailable");
    const report: { last_run: { state: string } } = await response.json();
    statusElement.textContent = report.last_run.state;
    if (report.last_run.state === "running") window.setTimeout(refreshStatus, 1000);
  } catch {
    statusElement.textContent = "Check status unavailable";
  }
}

async function runChecks(): Promise<void> {
  if (!statusElement) return;
  try {
    const response = await fetch("/checks", { method: "POST" });
    statusElement.textContent = response.ok ? "running" : "request rejected";
    if (response.ok) await refreshStatus();
  } catch {
    statusElement.textContent = "Check request failed";
  }
}

document.querySelector("#run")?.addEventListener("click", () => void runChecks());
document.querySelector("#refresh")?.addEventListener("click", () => window.location.reload());
void refreshStatus();
