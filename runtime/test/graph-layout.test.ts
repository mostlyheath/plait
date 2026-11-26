import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { test } from 'node:test';
import {
  layoutGraph,
  type Layout,
  type LayoutEdge,
  type LayoutNode,
  type Placed,
} from '../src/graph/layout.ts';

function createRng(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = Math.imul(state ^ (state >>> 15), 1 | state);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 0x100000000;
  };
}

function shuffle<T>(items: readonly T[], rng: () => number): T[] {
  const copy = items.slice();
  for (let i = copy.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    const tmp = copy[i];
    copy[i] = copy[j];
    copy[j] = tmp;
  }
  return copy;
}

function byId(layout: Layout): Map<string, Placed> {
  return new Map(layout.placed.map(item => [item.id, item]));
}

test('empty graph and single node lay out cleanly', () => {
  assert.deepEqual(layoutGraph([], []), { placed: [], layers: 0, columns: 0 });
  assert.deepEqual(layoutGraph([], [{ from: 'a', to: 'b', kind: 'fork' }]), {
    placed: [],
    layers: 0,
    columns: 0,
  });

  assert.deepEqual(layoutGraph([{ id: 'only', order: 0 }], []), {
    placed: [{ id: 'only', layer: 0, column: 0 }],
    layers: 1,
    columns: 1,
  });
});

test('a chain places one node per layer in column 0', () => {
  const nodes: LayoutNode[] = [
    { id: 'n0', order: 0 },
    { id: 'n1', order: 1 },
    { id: 'n2', order: 2 },
    { id: 'n3', order: 3 },
  ];
  const edges: LayoutEdge[] = [
    { from: 'n0', to: 'n1', kind: 'fork' },
    { from: 'n1', to: 'n2', kind: 'depends_on' },
    { from: 'n2', to: 'n3', kind: 'fork' },
  ];

  assert.deepEqual(layoutGraph(nodes, edges), {
    placed: [
      { id: 'n0', layer: 0, column: 0 },
      { id: 'n1', layer: 1, column: 0 },
      { id: 'n2', layer: 2, column: 0 },
      { id: 'n3', layer: 3, column: 0 },
    ],
    layers: 4,
    columns: 1,
  });
});

test('a fork with three children orders children by creation order in layer 1', () => {
  const nodes: LayoutNode[] = [
    { id: 'root', order: 10 },
    { id: 'c3', order: 30 },
    { id: 'c1', order: 15 },
    { id: 'c2', order: 20 },
  ];
  const edges: LayoutEdge[] = [
    { from: 'root', to: 'c3', kind: 'fork' },
    { from: 'root', to: 'c1', kind: 'fork' },
    { from: 'root', to: 'c2', kind: 'fork' },
  ];

  assert.deepEqual(layoutGraph(nodes, edges), {
    placed: [
      { id: 'root', layer: 0, column: 0 },
      { id: 'c1', layer: 1, column: 0 },
      { id: 'c2', layer: 1, column: 1 },
      { id: 'c3', layer: 1, column: 2 },
    ],
    layers: 2,
    columns: 3,
  });
});

test('diamond places the joined child one layer below both parents', () => {
  // Symmetric 4-node diamond: root -> (left, right) -> join.
  const diamondNodes: LayoutNode[] = [
    { id: 'root', order: 0 },
    { id: 'left', order: 1 },
    { id: 'right', order: 2 },
    { id: 'join', order: 3 },
  ];
  const diamondEdges: LayoutEdge[] = [
    { from: 'root', to: 'left', kind: 'fork' },
    { from: 'root', to: 'right', kind: 'fork' },
    { from: 'left', to: 'join', kind: 'depends_on' },
    { from: 'right', to: 'join', kind: 'depends_on' },
  ];

  assert.deepEqual(layoutGraph(diamondNodes, diamondEdges), {
    placed: [
      { id: 'root', layer: 0, column: 0 },
      { id: 'left', layer: 1, column: 0 },
      { id: 'right', layer: 1, column: 1 },
      { id: 'join', layer: 2, column: 0 },
    ],
    layers: 3,
    columns: 2,
  });

  // Two parents at different depths: child sits one layer below the deeper parent.
  const asymmetricNodes: LayoutNode[] = [
    { id: 'p1', order: 0 },
    { id: 'top', order: 1 },
    { id: 'p2', order: 2 },
    { id: 'child', order: 3 },
  ];
  const asymmetricEdges: LayoutEdge[] = [
    { from: 'top', to: 'p2', kind: 'fork' },
    { from: 'p1', to: 'child', kind: 'depends_on' },
    { from: 'p2', to: 'child', kind: 'depends_on' },
  ];
  const asymmetric = layoutGraph(asymmetricNodes, asymmetricEdges);
  const pos = byId(asymmetric);
  assert.equal(pos.get('p1')?.layer, 0);
  assert.equal(pos.get('top')?.layer, 0);
  assert.equal(pos.get('p2')?.layer, 1);
  assert.equal(pos.get('child')?.layer, 2);
  assert.equal(asymmetric.layers, 3);
});

test('mixed fork and depends_on pushes a forked sibling deeper when it waits on another', () => {
  const nodes: LayoutNode[] = [
    { id: 'root', order: 0 },
    { id: 'a', order: 1 },
    { id: 'b', order: 2 },
    { id: 'c', order: 3 },
  ];
  const edges: LayoutEdge[] = [
    { from: 'root', to: 'a', kind: 'fork' },
    { from: 'root', to: 'b', kind: 'fork' },
    { from: 'root', to: 'c', kind: 'fork' },
    { from: 'a', to: 'b', kind: 'depends_on' },
    { from: 'b', to: 'c', kind: 'depends_on' },
  ];

  assert.deepEqual(layoutGraph(nodes, edges), {
    placed: [
      { id: 'root', layer: 0, column: 0 },
      { id: 'a', layer: 1, column: 0 },
      { id: 'b', layer: 2, column: 0 },
      { id: 'c', layer: 3, column: 0 },
    ],
    layers: 4,
    columns: 1,
  });
});

test('layout is deterministic and independent of input node and edge order', () => {
  const nodes: LayoutNode[] = [
    { id: 'r1', order: 0 },
    { id: 'r2', order: 1 },
    { id: 'a', order: 2 },
    { id: 'b', order: 3 },
    { id: 'c', order: 4 },
    { id: 'd', order: 5 },
    { id: 'e', order: 6 },
    { id: 'f', order: 7 },
    { id: 'iso', order: 8 },
  ];
  const edges: LayoutEdge[] = [
    { from: 'r1', to: 'b', kind: 'fork' },
    { from: 'r2', to: 'a', kind: 'fork' },
    { from: 'r1', to: 'c', kind: 'fork' },
    { from: 'a', to: 'd', kind: 'depends_on' },
    { from: 'b', to: 'd', kind: 'depends_on' },
    { from: 'b', to: 'e', kind: 'fork' },
    { from: 'c', to: 'f', kind: 'depends_on' },
    { from: 'd', to: 'f', kind: 'depends_on' },
  ];

  const nodesSnapshot = nodes.slice();
  const edgesSnapshot = edges.slice();
  const baseline = layoutGraph(nodes, edges);
  assert.deepEqual(nodes, nodesSnapshot);
  assert.deepEqual(edges, edgesSnapshot);

  const rng = createRng(0x5eed1234);
  for (let trial = 0; trial < 20; trial++) {
    const shuffledNodes = shuffle(nodes, rng);
    const shuffledEdges = shuffle(edges, rng);
    assert.deepEqual(layoutGraph(shuffledNodes, shuffledEdges), baseline);
  }
});

test('missing endpoints, self-edges, and duplicate edges are ignored', () => {
  const nodes: LayoutNode[] = [
    { id: 'a', order: 0 },
    { id: 'b', order: 1 },
    { id: 'c', order: 2 },
  ];
  const cleanEdges: LayoutEdge[] = [
    { from: 'a', to: 'b', kind: 'fork' },
    { from: 'b', to: 'c', kind: 'depends_on' },
  ];
  const dirtyEdges: LayoutEdge[] = [
    ...cleanEdges,
    { from: 'missing', to: 'a', kind: 'fork' },
    { from: 'c', to: 'missing', kind: 'depends_on' },
    { from: 'ghost1', to: 'ghost2', kind: 'fork' },
    { from: 'a', to: 'a', kind: 'fork' },
    { from: 'b', to: 'b', kind: 'depends_on' },
    { from: 'a', to: 'b', kind: 'fork' },
    { from: 'a', to: 'b', kind: 'depends_on' },
    { from: 'b', to: 'c', kind: 'depends_on' },
  ];

  assert.deepEqual(layoutGraph(nodes, dirtyEdges), layoutGraph(nodes, cleanEdges));
});

test('cycles never hang and produce a deterministic layout regardless of input order', () => {
  const nodes: LayoutNode[] = [
    { id: 'a', order: 0 },
    { id: 'b', order: 1 },
    { id: 'c', order: 2 },
    { id: 'tail', order: 3 },
  ];
  const edges: LayoutEdge[] = [
    { from: 'a', to: 'b', kind: 'depends_on' },
    { from: 'b', to: 'c', kind: 'depends_on' },
    { from: 'c', to: 'a', kind: 'depends_on' },
    { from: 'b', to: 'tail', kind: 'fork' },
  ];

  const baseline = layoutGraph(nodes, edges);
  assert.equal(baseline.placed.length, 4);
  assert.deepEqual(baseline, {
    placed: [
      { id: 'a', layer: 0, column: 0 },
      { id: 'b', layer: 1, column: 0 },
      { id: 'c', layer: 2, column: 0 },
      { id: 'tail', layer: 2, column: 1 },
    ],
    layers: 3,
    columns: 2,
  });

  const rng = createRng(0xc1c1e5);
  for (let trial = 0; trial < 10; trial++) {
    assert.deepEqual(layoutGraph(shuffle(nodes, rng), shuffle(edges, rng)), baseline);
  }
});

test('barycenter sweeps uncross children listed crosswise and reorder parents when needed', () => {
  // Children listed crosswise relative to their parents: down sweep uncrosses layer 1.
  const crossNodes: LayoutNode[] = [
    { id: 'p1', order: 0 },
    { id: 'p2', order: 1 },
    { id: 'c2', order: 2 },
    { id: 'c1', order: 3 },
  ];
  const crossEdges: LayoutEdge[] = [
    { from: 'p1', to: 'c1', kind: 'fork' },
    { from: 'p2', to: 'c2', kind: 'fork' },
  ];

  const crossLayout = layoutGraph(crossNodes, crossEdges);
  const crossPos = byId(crossLayout);
  assert.equal(crossPos.get('p1')?.column, crossPos.get('c1')?.column);
  assert.equal(crossPos.get('p2')?.column, crossPos.get('c2')?.column);
  assert.deepEqual(crossLayout, {
    placed: [
      { id: 'p1', layer: 0, column: 0 },
      { id: 'p2', layer: 0, column: 1 },
      { id: 'c1', layer: 1, column: 0 },
      { id: 'c2', layer: 1, column: 1 },
    ],
    layers: 2,
    columns: 2,
  });

  // Up sweep brings p2 next to p0 so p2 -> shared does not cross p1 -> right.
  const upNodes: LayoutNode[] = [
    { id: 'p0', order: 0 },
    { id: 'p1', order: 1 },
    { id: 'p2', order: 2 },
    { id: 'left', order: 3 },
    { id: 'shared', order: 4 },
    { id: 'right', order: 5 },
  ];
  const upEdges: LayoutEdge[] = [
    { from: 'p0', to: 'left', kind: 'fork' },
    { from: 'p0', to: 'shared', kind: 'depends_on' },
    { from: 'p2', to: 'shared', kind: 'depends_on' },
    { from: 'p1', to: 'right', kind: 'fork' },
  ];

  const upPos = byId(layoutGraph(upNodes, upEdges));
  assert.equal(upPos.get('p0')?.column, 0);
  assert.equal(upPos.get('p2')?.column, 1);
  assert.equal(upPos.get('p1')?.column, 2);
  assert.equal(upPos.get('left')?.column, 0);
  assert.equal(upPos.get('shared')?.column, 1);
  assert.equal(upPos.get('right')?.column, 2);
});

test('adding an unrelated isolated node never moves existing cards in other layers', () => {
  const baseNodes: LayoutNode[] = [
    { id: 'p0', order: 1 },
    { id: 'p1', order: 3 },
    { id: 'p2', order: 5 },
    { id: 'c0', order: 6 },
    { id: 'c1', order: 7 },
    { id: 'c2', order: 8 },
    { id: 'c3', order: 9 },
    { id: 'c4', order: 10 },
  ];
  const baseEdges: LayoutEdge[] = [
    { from: 'p0', to: 'c0', kind: 'fork' },
    { from: 'p0', to: 'c1', kind: 'fork' },
    { from: 'p2', to: 'c1', kind: 'depends_on' },
    { from: 'p1', to: 'c2', kind: 'fork' },
    { from: 'p2', to: 'c3', kind: 'fork' },
    { from: 'p2', to: 'c4', kind: 'fork' },
  ];

  const base = layoutGraph(baseNodes, baseEdges);
  const baseLayer0Count = base.placed.filter(p => p.layer === 0).length;

  // Highest-order isolated node appends at the end of layer 0 and leaves every existing card untouched.
  const withHigh = layoutGraph([...baseNodes, { id: 'iso-high', order: 100 }], baseEdges);
  const highPos = byId(withHigh);
  assert.deepEqual(highPos.get('iso-high'), { id: 'iso-high', layer: 0, column: baseLayer0Count });
  for (const existing of base.placed) {
    assert.deepEqual(highPos.get(existing.id), existing);
  }

  // Lower-order isolated node also leaves every existing card in other layers untouched.
  const withLow = layoutGraph([...baseNodes, { id: 'iso-low', order: 0 }], baseEdges);
  const lowPos = byId(withLow);
  for (const existing of base.placed) {
    if (existing.layer > 0) {
      assert.deepEqual(lowPos.get(existing.id), existing);
    }
  }
});

test('500 nodes and 2,000 seeded acyclic edges lay out in under 5 ms with valid invariants', () => {
  const nodeCount = 500;
  const edgeCount = 2000;
  const rng = createRng(0x5002000);

  const nodes: LayoutNode[] = Array.from({ length: nodeCount }, (_, i) => ({
    id: `t-${i}`,
    order: i,
  }));

  const seen = new Set<number>();
  const edges: LayoutEdge[] = [];
  while (edges.length < edgeCount) {
    const a = Math.floor(rng() * nodeCount);
    const b = Math.floor(rng() * nodeCount);
    if (a === b) continue;
    const from = a < b ? a : b;
    const to = a < b ? b : a;
    const key = from * nodeCount + to;
    if (seen.has(key)) continue;
    seen.add(key);
    edges.push({
      from: nodes[from].id,
      to: nodes[to].id,
      kind: rng() < 0.5 ? 'fork' : 'depends_on',
    });
  }

  // Warm up once before timing.
  const warmup = layoutGraph(nodes, edges);

  const start = performance.now();
  const layout = layoutGraph(nodes, edges);
  const elapsedMs = performance.now() - start;

  assert.ok(elapsedMs < 5, `Expected 500-node / 2,000-edge layout under 5 ms, took ${elapsedMs.toFixed(2)} ms`);
  assert.deepEqual(layout, warmup);
  assert.equal(layout.placed.length, nodeCount);

  const pos = byId(layout);
  assert.equal(pos.size, nodeCount);

  // Every edge goes strictly downward, and every non-root node has a predecessor one layer above.
  const maxPredLayer = new Map<string, number>();
  for (const edge of edges) {
    const fromPos = pos.get(edge.from);
    const toPos = pos.get(edge.to);
    assert.ok(fromPos && toPos);
    assert.ok(toPos.layer > fromPos.layer, `Expected ${edge.to} below ${edge.from}`);
    const prev = maxPredLayer.get(edge.to) ?? -1;
    if (fromPos.layer > prev) {
      maxPredLayer.set(edge.to, fromPos.layer);
    }
  }

  const perLayer = new Map<number, number[]>();
  let maxLayer = 0;
  for (const item of layout.placed) {
    const expectedLayer = (maxPredLayer.get(item.id) ?? -1) + 1;
    assert.equal(item.layer, expectedLayer);
    if (item.layer > maxLayer) maxLayer = item.layer;
    const cols = perLayer.get(item.layer) ?? [];
    cols.push(item.column);
    perLayer.set(item.layer, cols);
  }

  assert.equal(layout.layers, maxLayer + 1);
  let widest = 0;
  for (let l = 0; l < layout.layers; l++) {
    const cols = (perLayer.get(l) ?? []).slice().sort((a, b) => a - b);
    assert.ok(cols.length > 0, `Layer ${l} should be non-empty`);
    assert.deepEqual(
      cols,
      Array.from({ length: cols.length }, (_, c) => c),
    );
    if (cols.length > widest) widest = cols.length;
  }
  assert.equal(layout.columns, widest);
});
