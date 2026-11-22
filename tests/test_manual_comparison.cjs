const assert = require('node:assert/strict');
const {readFileSync} = require('node:fs');
const vm = require('node:vm');

const app = readFileSync('dashboard/app.js', 'utf8');
const script = app.slice(app.indexOf('function showDiscrepancies()'), app.indexOf('var inventoryStream'));
const elements = new Map();
const node = () => ({hidden:true,dataset:{items:'[]'}, classList:{toggle(){}}, replaceChildren(){}});
const $ = key => {
  if (!elements.has(key)) elements.set(key, node());
  return elements.get(key);
};
const accuracy = {checked:101, pending:false, hosts:[], findings:[], coverage:{verified:6, expected:6, unconfigured:0}};
const data = {fetched_at:null, error:null, inventory_accuracy:accuracy};
let accept;
const posted = new Promise(resolve => { accept = resolve; });
const browser = vm.createContext({
  $, now:()=>100, data, healthFindings:()=>[], leaseFindings:()=>[], reconcileRequested:0,
  document:{createDocumentFragment:()=>({})}, findingGroups:[], AbortSignal,
  fetch:()=>posted, setTimeout:()=>{}, age:()=>'', Date,
});
vm.runInContext(script, browser);
(async () => {
  $('reconcile').onclick();
  browser.showDiscrepancies();
  assert.doesNotMatch($('reconcile-status').textContent, /Comparison finished/);
  accept({ok:true, json:async()=>({id:'this-comparison'})});
  await new Promise(setImmediate);
  data.comparison = {id:'other-comparison', state:'complete'};
  browser.showDiscrepancies();
  assert.doesNotMatch($('reconcile-status').textContent, /Comparison finished/);
  data.comparison = {id:'this-comparison', state:'complete'};
  browser.showDiscrepancies();
  assert.match($('reconcile-status').textContent, /Comparison finished.*6 of 6/);
  console.log('PASS manual comparison waits for its own accepted result');
})().catch(error => { console.error(error); process.exitCode = 1; });
