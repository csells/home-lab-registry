# Automatic resource graph layout

Decision: use **ELK.js 0.12.1 directly**, with ELK Layered and orthogonal edge
routing. Keep native SVG/HTML rendering, existing detail cards, and separate
private inventories. Research accessed 2026-11-09; implementation verified
2026-11-10. This is a choice for this registry, not a universal library ranking.

## Requirements and evidence

The graph contains physical machines as peers, nested guests, and services inside
either host or guest. Directed dependencies cross containment boundaries; some
connect a guest to its own physical host and others form cycles. The previous
CSS grid and manually drawn curves did not reserve routing space, so connections
overlapped resources. Layout must arrange groups and routes together. The app is
vanilla JavaScript served by Python, with no existing React build pipeline.

Reviewed primary documentation, adapter source, release metadata, and exact npm
package contents. Tested ELK against synthetic nested/cyclic/ancestor-edge cases
and the private captured work-lab catalog. Real inventory stays outside Git.

## Comparison

| Candidate | Verified capabilities and limitations | Fit here |
| --- | --- | --- |
| [ELK.js](https://github.com/kieler/elkjs) / [Layered](https://eclipse.dev/elk/reference/algorithms/org-eclipse-elk-layered.html) | Compound graphs, cross-hierarchy edges, orthogonal routing, ports, labels; browser Worker support. Layout engine only. | Best match: consume both coordinates and routed edge sections without changing UI framework. |
| [Cytoscape + fCoSE](https://github.com/iVis-at-Bilkent/cytoscape.js-fcose) | Compound force layout, fixed/alignment/relative constraints, incremental refinement. Does not supply ELK-style obstacle-aware orthogonal routes. | Useful for exploratory networks; directional architecture and connection routing are our priority. |
| [Cytoscape + ELK](https://github.com/cytoscape/cytoscape.js-elk/blob/master/src/layout.js) | Adapter's `run()` transfers leaf positions through `layoutPositions`; it does not consume ELK `sections`/bendpoints. | Would discard the routing geometry we need, or require extra custom integration. |
| [Dagre](https://github.com/dagrejs/dagre) | Lightweight layered directed layout. [React Flow's official comparison](https://reactflow.dev/learn/layouting/layouting) identifies subflow issues when nested nodes connect outside their group. | Our central relationship shape hits that limitation. |
| [Graphviz dot](https://graphviz.org/docs/attrs/compound/) / [Viz.js](https://github.com/mdaines/viz-js) | Clusters and directed routing; browser WASM available. [Orthogonal spline docs](https://graphviz.org/docs/attrs/splines/) explicitly note limitations for ports and dot edge labels. | Credible alternative, with DOT/WASM or server integration costs; ELK fits the existing JS data path more directly. |
| [React Flow](https://reactflow.dev/learn/layouting/layouting) | Interactive React diagram renderer; no built-in automatic layout engine. ELK remains an external integration. | Adds React and a new rendering architecture without replacing the layout requirement. |

These are capability comparisons, not measured performance rankings. No broad
benchmark or production-scale capacity claim was made for the rejected engines.

## Integration details

- Pin the [released 0.12.1](https://github.com/kieler/elkjs/releases/tag/0.12.1),
  not the unreleased version in master. [npm metadata](https://registry.npmjs.org/elkjs/0.12.1)
  dates this release to 2026-10-28.
- Vendor `elk-api.js` (9,782 bytes) and `elk-worker.min.js` (1,631,868 bytes).
  Measured gzip sizes: 2,944 and 477,062 bytes. The combined browser bundle is
  1,646,241 bytes. A local API/Worker split keeps layout off the UI thread and
  avoids runtime CDN access. Sizes describe exact upstream files, not an app benchmark.
- Preserve upstream notices and `LICENSE.md`. The package offers
  `EPL-2.0 OR GPL-3.0-or-later`; distribute under the EPL option.
- Use `elk.hierarchyHandling=INCLUDE_CHILDREN`, `elk.edgeRouting=ORTHOGONAL`,
  stable sorted input, and nonzero `elk.randomSeed=42`.
  [Hierarchy handling](https://eclipse.dev/elk/reference/options/org-eclipse-elk-hierarchyHandling.html),
  [model order](https://eclipse.dev/elk/reference/options/org-eclipse-elk-layered-considerModelOrder-strategy.html),
  and [random seed](https://eclipse.dev/elk/reference/options/org-eclipse-elk-randomSeed.html)
  describe the relevant behavior. Cache geometry and preserve pan/zoom when only
  status changes. Sorting and caching reduce churn; topology changes can move nodes.
- Put each edge in its lowest common ancestor. A child-to-ancestor edge belongs
  inside that ancestor. This follows [maintainer guidance](https://github.com/eclipse-elk/elk/issues/1068#issuecomment-2301261006)
  and passed the 0.12.1 spike. Do not translate dependencies into physical placement.
- Request `elk.json.edgeCoords=ROOT`; consume returned bendpoints directly.
  Node coordinates remain parent-relative and are accumulated during rendering.
  See [coordinate systems](https://eclipse.dev/elk/documentation/tooldevelopers/graphdatastructure/coordinatesystem.html)
  and [JSON edge coordinates](https://eclipse.dev/elk/reference/options/org-eclipse-elk-json-edgeCoords.html).
- Start with implicit ports. [Nested fixed-order port issue 1192](https://github.com/eclipse-elk/elk/issues/1192)
  is a caution from older versions, not a reproduced defect in the chosen release.
- Use compact resource headers and retain full operational details in existing
  cards. Keep a readable default scale, with explicit Fit, zoom, pan and
  fullscreen controls rather than making every label tiny to fit everything.

## Validation and limits

`tests/test_resource_graph.cjs` exercises ELK itself: physical peers, invalid
placement and cycles, cross-container dependencies, ancestor edges, stable input
ordering, filter context, and orthogonal route segments. The browser suite uses
the real dashboard static routes and CSP, including loading the local Worker;
it exercises navigation, pan/zoom, filters, keyboard controls and existing cards.
The actual staged service is also checked using the private captured inventory.

ELK is heuristic. It can still produce crossings and large groups; this change
does not claim a globally optimal or crossing-free layout. Very large inventories
and browsers other than the tested Chromium are not qualified by these checks.
Declared arrows are catalog relationships, not traffic or connectivity monitoring.
