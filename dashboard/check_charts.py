"""Rendered chart regression against an authenticated disposable dashboard.

Usage: LAB_LOGIN_JSON=/private/login.json python check_charts.py https://preview.example
Requires Playwright and its Chromium browser. Optionally set LAB_CHROME_EXECUTABLE
for a separate Chrome for Testing installation. Never use a user's Chrome binary.
Fixtures exercise the real renderer; they are not fleet measurements.
"""
import json
import os
import sys
from pathlib import Path
from playwright.sync_api import sync_playwright

with sync_playwright() as playwright:
    options = {"headless": True}
    if os.environ.get("LAB_CHROME_EXECUTABLE"):
        options["executable_path"] = os.environ["LAB_CHROME_EXECUTABLE"]
    browser = playwright.chromium.launch(**options)
    try:
        context = browser.new_context(
            http_credentials=json.loads(Path(os.environ["LAB_LOGIN_JSON"]).read_text()),
            viewport={"width": 393, "height": 900},
        )
        page = context.new_page()
        page.goto(sys.argv[1], wait_until="domcontentloaded")
        page.wait_for_function("typeof trend === 'function'")
        result = page.evaluate("""() => {
          const end=now(), root=document.createElement('section');
          root.className='telemetry-strip';root.style.width='340px';
          const grid=document.createElement('div');grid.className='telemetry-charts';
          root.append(grid);document.body.append(root);
          const rows=[['CPU',2,4],['RAM',79,81],['DISK',22,22]];
          const results=rows.map(([label,low,high])=>{
            const samples=Array.from({length:30},(_,i)=>({checked:end-590+i*20,status:'available',value:i%2?low:high}));
            const cell=document.createElement('div');cell.className='telemetry-cell';
            const chart=trend(samples,s=>s.value,label);cell.append(chart);grid.append(cell);
            const heights=[...chart.querySelectorAll('.history-bar')].map(b=>b.getBoundingClientRect().height);
            return {label,range:chart.dataset.range,spread:Math.max(...heights)-Math.min(...heights),count:heights.length};
          });root.remove();return results;
        }""")
        assert result[0]["spread"] > 5, result
        assert result[1]["spread"] > 5, result
        assert result[2]["spread"] == 0, result
        assert [x["range"] for x in result] == ["0–10%", "75–85%", "0–100%"], result
        assert all(x["count"] == 30 for x in result), result
        print("PASS changing CPU/RAM visibly differ; constant disk stays flat; explicit scales", result)
        context.close()
    finally:
        browser.close()
