import ELK from 'elkjs/lib/elk-api.js';
import type { ElkExtendedEdge, ElkNode, LayoutOptions } from 'elkjs';

export interface GraphRecord {
  id: string;
  name: string;
  kind?: string;
  machine_id?: string | null;
  dependencies?: string[];
  status?: string;
}

export interface GraphNode extends ElkNode {
  recordId?: string;
  children?: GraphNode[];
}

interface Dependency {
  id: string;
  source: string;
  target: string;
}

const unplacedKey = Symbol('unplaced');
type Parent = string | typeof unplacedKey;

const groupPadding = { 'elk.padding': '[top=88,left=28,bottom=28,right=28]' };
const layoutOptions: LayoutOptions = {
  'elk.algorithm': 'layered',
  'elk.direction': 'RIGHT',
  'elk.edgeRouting': 'ORTHOGONAL',
  'elk.hierarchyHandling': 'INCLUDE_CHILDREN',
  'elk.randomSeed': '42',
  'elk.spacing.nodeNode': '36',
  'elk.spacing.edgeNode': '24',
  'elk.layered.spacing.nodeNodeBetweenLayers': '70',
  'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
  'elk.json.edgeCoords': 'ROOT',
  'elk.padding': '[top=24,left=24,bottom=24,right=24]',
};

export function buildResourceGraph(input: readonly GraphRecord[], matching?: ReadonlySet<string>) {
  const ordered = [...input].sort((left, right) => left.id.localeCompare(right.id));
  const all = new Map(ordered.map(record => [record.id, record]));
  const parents = new Map<string, Parent>();
  for (const record of all.values()) {
    if (record.kind === 'machine') continue;
    const seen = new Set([record.id]);
    const immediate = all.get(record.machine_id ?? '');
    let ancestor = immediate;
    while (ancestor && ancestor.kind !== 'machine' && !seen.has(ancestor.id)) {
      seen.add(ancestor.id);
      ancestor = all.get(ancestor.machine_id ?? '');
    }
    parents.set(record.id, ancestor?.kind === 'machine' && immediate && immediate.kind !== 'service'
      ? immediate.id : unplacedKey);
  }

  const included = new Set<string>();
  for (const record of all.values()) {
    if (matching && !matching.has(record.id)) continue;
    let id: Parent | undefined = record.id;
    while (typeof id === 'string' && all.has(id) && !included.has(id)) {
      included.add(id);
      id = parents.get(id);
    }
  }
  const records = new Map([...all].filter(([id]) => included.has(id)));
  const children: GraphNode[] = [];
  const graph: GraphNode = { id: 'root', children, edges: [], layoutOptions };
  const nodes = new Map<string, GraphNode>();
  let index = 0;
  for (const record of records.values()) {
    nodes.set(record.id, {
      id: 'n' + index++, recordId: record.id, width: 280,
      height: record.kind === 'service' ? 84 : 100, edges: [],
    });
  }

  const unplaced: GraphNode = { id: 'unplaced', edges: [] };
  function contain(container: GraphNode, node: GraphNode) {
    if (container !== graph && !container.children) {
      container.layoutOptions = container === unplaced ? groupPadding : {
        ...groupPadding,
        'elk.nodeSize.constraints': 'MINIMUM_SIZE',
        'elk.nodeSize.minimum': '(300,100)',
      };
      if (container !== unplaced) container.height = 84;
    }
    (container.children ??= []).push(node);
  }
  for (const [id, node] of nodes) {
    const parent = parents.get(id);
    if (parent === unplacedKey) contain(unplaced, node);
    else contain(nodes.get(parent ?? '') ?? graph, node);
  }
  if (unplaced.children?.length) children.push(unplaced);

  function ancestors(id: string) {
    const path: Parent[] = [];
    let current: Parent | undefined = id;
    while (current !== undefined) {
      path.push(current);
      current = current === unplacedKey ? undefined : parents.get(current);
    }
    return path;
  }

  const edges: Dependency[] = [];
  const missing: Dependency[] = [];
  for (const record of records.values()) {
    for (const target of [...new Set(record.dependencies)].sort()) {
      if (target === record.id) continue;
      const edge = { id: 'e' + edges.length, source: record.id, target };
      const targetNode = nodes.get(target);
      if (!targetNode) {
        missing.push(edge);
        continue;
      }
      const targetPath = new Set(ancestors(target));
      const common = ancestors(record.id).find(id => targetPath.has(id));
      const container = common === unplacedKey ? unplaced : nodes.get(common ?? '') ?? graph;
      const sourceNode = nodes.get(record.id);
      if (!sourceNode) throw new Error('Resource graph is missing a source node.');
      (container.edges ??= []).push({ id: edge.id, sources: [sourceNode.id], targets: [targetNode.id] });
      edges.push(edge);
    }
  }
  return { graph, records, edges, missing, unplaced: unplaced.children?.length ?? 0 };
}

type GraphInput = ReturnType<typeof buildResourceGraph>;
interface View {
  signature: string;
  dispose: () => void;
  layout?: GraphNode;
}

const views = new WeakMap<HTMLElement, View>();
let engine: InstanceType<typeof ELK> | undefined;

function element<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string) {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function svg<K extends keyof SVGElementTagNameMap>(tag: K, attributes: Record<string, number | string> = {}) {
  const node = document.createElementNS('http://www.w3.org/2000/svg', tag);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  return node;
}

const kindLabels: Record<string, string> = {
  machine: 'Physical machine',
  'virtual-machine': 'Virtual machine',
  service: 'Application service',
  unplaced: 'Placement unknown',
};

function render(canvas: SVGGElement, result: GraphNode, input: GraphInput) {
  const defs = svg('defs');
  const marker = svg('marker', {
    id: 'resource-arrow', viewBox: '0 0 10 10', refX: 9, refY: 5,
    markerWidth: 7, markerHeight: 7, orient: 'auto',
  });
  marker.append(svg('path', { d: 'M 0 0 L 10 5 L 0 10 z', class: 'graph-arrow' }));
  defs.append(marker);
  const backgrounds = svg('g');
  const routes = svg('g');
  const headers = svg('g');
  canvas.replaceChildren(defs, backgrounds, routes, headers);
  const dependencies = new Map(input.edges.map(edge => [edge.id, edge]));

  function route(edge: ElkExtendedEdge) {
    const dependency = dependencies.get(edge.id);
    if (!dependency) throw new Error('Resource layout returned an unknown dependency.');
    const source = input.records.get(dependency.source);
    const target = input.records.get(dependency.target);
    if (!source || !target) throw new Error('Resource layout returned an unrecognized endpoint.');
    for (const section of edge.sections ?? []) {
      const points = [section.startPoint, ...(section.bendPoints ?? []), section.endPoint];
      const path = svg('path', {
        d: points.map((point, index) => (index ? 'L' : 'M') + point.x + ' ' + point.y).join(' '),
        class: 'resource-edge', 'marker-end': 'url(#resource-arrow)',
      });
      const title = svg('title');
      title.textContent = source.name + ' → ' + target.name;
      path.append(title);
      routes.append(path);
    }
  }

  function visit(node: GraphNode, offsetX = 0, offsetY = 0) {
    const x = offsetX + (node.x ?? 0);
    const y = offsetY + (node.y ?? 0);
    const record = input.records.get(node.recordId ?? '');
    if (node.id !== 'root') {
      const kind = record?.kind || 'unplaced';
      const width = node.width ?? 0;
      const height = node.height ?? 0;
      const boundary = svg('rect', { x, y, width, height, rx: 10, class: 'resource-boundary resource-' + kind });
      boundary.dataset.resourceBox = record?.id || 'unplaced';
      backgrounds.append(boundary);
      const foreign = svg('foreignObject', {
        x: x + 8, y: y + 8, width: width - 16, height: node.children?.length ? 64 : height - 16,
      });
      const label = element('div', 'resource-node resource-' + kind);
      if (record) {
        label.dataset.resourceId = record.id;
        label.title = record.name;
      }
      label.append(
        element('span', 'resource-kind', kindLabels[kind] ?? 'Container'),
        element('strong', 'resource-label', record?.name || 'Unplaced resources'),
      );
      if (record) label.append(element('span', 'resource-state', record.status || 'Unknown'));
      foreign.append(label);
      headers.append(foreign);
    }
    for (const child of node.children ?? []) visit(child, x, y);
    for (const edge of node.edges ?? []) route(edge);
  }

  visit(result);
}

function message(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function mountResourceGraph(root: HTMLElement, records: GraphRecord[], matching: ReadonlySet<string>) {
  const previous = views.get(root);
  previous?.dispose();
  const input = buildResourceGraph(records, matching);
  let disposed = false;
  let deadline: ReturnType<typeof setTimeout> | undefined;
  const view: View = {
    signature: JSON.stringify(input.graph),
    dispose() {
      disposed = true;
      clearTimeout(deadline);
    },
  };
  views.set(root, view);

  root.classList.add('resource-graph');
  root.dataset.layoutReady = 'false';
  const status = element('p', 'graph-status', 'Arranging resources...');
  status.setAttribute('role', 'status');
  const viewport = element('div', 'graph-viewport');
  const drawing = svg('svg', { 'aria-label': 'Resource containment and declared dependencies', role: 'img' });
  const canvas = svg('g');
  drawing.append(canvas);
  viewport.append(drawing);
  root.replaceChildren(status, viewport,
    element('p', 'graph-legend', 'Boxes contain their resources. Dashed arrows point from caller to declared dependency.'));

  function fail(reason: string) {
    clearTimeout(deadline);
    if (disposed) return;
    status.textContent = reason;
    status.setAttribute('role', 'alert');
  }

  function finish(layout: GraphNode) {
    if (disposed) return;
    clearTimeout(deadline);
    view.layout = layout;
    const pad = 18;
    drawing.style.width = (layout.width ?? 0) + 2 * pad + 'px';
    drawing.style.height = (layout.height ?? 0) + 2 * pad + 'px';
    canvas.setAttribute('transform', `translate(${pad} ${pad})`);
    render(canvas, layout, input);
    const summaries = [`${input.records.size} resources`, `${input.edges.length} declared connections`];
    if (input.unplaced) summaries.push(`${input.unplaced} unplaced`);
    if (input.missing.length) summaries.push(`${input.missing.length} targets outside this view`);
    status.textContent = summaries.join(' · ');
    status.setAttribute('role', 'status');
    root.dataset.layoutReady = 'true';
  }

  if (!input.records.size) {
    status.textContent = 'No resources match these filters.';
    viewport.hidden = true;
    root.dataset.layoutReady = 'true';
    return view.dispose;
  }

  try {
    if (previous?.layout && previous.signature === view.signature) finish(previous.layout);
    else {
      deadline = setTimeout(() => fail('Resource layout did not finish. Reload to retry.'), 30000);
      engine ??= new ELK({ workerUrl: new URL('vendor/elk-worker.min.js', document.baseURI).href });
      engine.layout(input.graph).then(finish).catch(error => fail('Resource layout failed: ' + message(error)));
    }
  } catch (error) {
    fail('Resource layout unavailable: ' + message(error));
  }
  return view.dispose;
}
