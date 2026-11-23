import assert from 'node:assert/strict';
import { test } from 'node:test';
import ELK from 'elkjs';
import { buildResourceGraph } from '../resource-graph.ts';
import type { GraphNode, GraphRecord } from '../resource-graph.ts';

const records: GraphRecord[] = [
  { id: 'host', name: 'Host', kind: 'machine' },
  { id: 'peer', name: 'Peer Mac', kind: 'machine', machine_id: 'host' },
  { id: 'a', name: 'Office guest', kind: 'container', machine_id: 'host', dependencies: ['host'] },
  { id: 'b', name: 'GC guest', kind: 'virtual-machine', machine_id: 'host' },
  { id: 'office', name: 'Office', kind: 'service', machine_id: 'a', dependencies: ['gc'] },
  { id: 'gc', name: 'Gas City', kind: 'service', machine_id: 'b', dependencies: ['office'] },
  { id: 'lost', name: 'Unknown placement', kind: 'container', machine_id: 'missing' },
  { id: 'cycle1', name: 'Cycle one', kind: 'container', machine_id: 'cycle2' },
  { id: 'cycle2', name: 'Cycle two', kind: 'container', machine_id: 'cycle1' },
];

test('ELK lays out physical peers, nested resources and orthogonal links at their common ancestor', async () => {
  const { graph, edges, unplaced } = buildResourceGraph(records);
  assert(graph.children?.some(node => node.recordId === 'peer'));
  assert.equal(unplaced, 3);
  assert.equal(edges.length, 3);
  assert.equal(graph.children?.find(node => node.recordId === 'host')?.edges?.length, 3);
  const result = await new ELK().layout(graph);
  assert(result.width && result.height);
  const seen = new Set<string>();
  let edgeCount = 0;
  function walk(node: GraphNode) {
    if (node.recordId) seen.add(node.recordId);
    for (const child of node.children ?? []) walk(child);
    for (const edge of node.edges ?? []) {
      edgeCount += 1;
      assert(edge.sections?.length);
      for (const section of edge.sections) {
        const points = [section.startPoint, ...(section.bendPoints ?? []), section.endPoint];
        for (let index = 1; index < points.length; index++) {
          const current = points[index];
          const prior = points[index - 1];
          assert(current.x === prior.x || current.y === prior.y);
        }
      }
    }
  }
  walk(result);
  assert.deepEqual(seen, new Set(records.map(record => record.id)));
  assert.equal(edgeCount, 3);
});

test('graph order is deterministic and filters retain ancestors while counting omitted dependencies', () => {
  assert.deepEqual(buildResourceGraph([...records].reverse()).graph, buildResourceGraph(records).graph);
  const filtered = buildResourceGraph(records, new Set(['office']));
  assert.deepEqual(new Set(filtered.records.keys()), new Set(['office', 'a', 'host']));
  assert.equal(filtered.missing.length, 1);
  assert.equal(buildResourceGraph(records, new Set()).records.size, 0);
});
