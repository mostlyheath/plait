// Pure layered graph layout for a project's thread graph.
//
// Layers follow longest paths from roots over both `fork` and `depends_on` edges,
// so every node sits one layer below its deepest predecessor. Within each layer,
// deterministic barycenter sweeps reduce edge crossings with exact rational
// comparisons and creation-order tie-breaking.

export type LayoutNode = { id: string; order: number }; // order: stable creation order, unique
export type LayoutEdge = { from: string; to: string; kind: 'fork' | 'depends_on' };
// fork: from = parent, to = child. depends_on: from = the thread waited for, to = the waiting thread.
// Either way `to` is drawn below `from`.
export type Placed = { id: string; layer: number; column: number };
export type Layout = { placed: Placed[]; layers: number; columns: number };

const BARYCENTER_SWEEPS = 4;

type CanonicalNodes = {
  ordered: LayoutNode[];
  idToIndex: Map<string, number>;
};

type LayerAssignment = {
  layer: Int32Array;
  layerCount: number;
  complete: boolean;
};

function canonicalizeNodes(nodes: LayoutNode[]): CanonicalNodes {
  // Sorting up front makes node indices 0..n-1 match creation order regardless of input order.
  const sorted = nodes.slice().sort((a, b) => {
    if (a.order !== b.order) return a.order < b.order ? -1 : 1;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });

  const ordered: LayoutNode[] = [];
  const idToIndex = new Map<string, number>();
  for (let i = 0; i < sorted.length; i++) {
    const node = sorted[i];
    if (idToIndex.has(node.id)) continue;
    idToIndex.set(node.id, ordered.length);
    ordered.push(node);
  }
  return { ordered, idToIndex };
}

function createAdjacency(nodeCount: number): number[][] {
  const adj = new Array<number[]>(nodeCount);
  for (let i = 0; i < nodeCount; i++) {
    adj[i] = [];
  }
  return adj;
}

function buildOutAdjacency(
  nodeCount: number,
  edges: LayoutEdge[],
  idToIndex: Map<string, number>,
): number[][] {
  const outAdj = createAdjacency(nodeCount);
  for (let i = 0; i < edges.length; i++) {
    const edge = edges[i];
    const from = idToIndex.get(edge.from);
    if (from === undefined) continue;
    const to = idToIndex.get(edge.to);
    if (to === undefined || from === to) continue;
    outAdj[from].push(to);
  }

  // Sort and deduplicate each list once so every downstream pass is input-order-independent.
  for (let u = 0; u < nodeCount; u++) {
    const list = outAdj[u];
    if (list.length <= 1) continue;
    list.sort((a, b) => a - b);
    let write = 1;
    for (let read = 1; read < list.length; read++) {
      if (list[read] !== list[write - 1]) {
        list[write++] = list[read];
      }
    }
    list.length = write;
  }

  return outAdj;
}

function breakCycles(nodeCount: number, outAdj: number[][]): void {
  // Iterative 3-color DFS visiting nodes and targets in creation order so back edges
  // are removed deterministically without risking call-stack overflow on deep graphs.
  const state = new Uint8Array(nodeCount); // 0 = unvisited, 1 = active, 2 = done
  const nodeStack = new Int32Array(nodeCount);
  const edgeIdxStack = new Int32Array(nodeCount);

  for (let start = 0; start < nodeCount; start++) {
    if (state[start] !== 0) continue;

    let top = 0;
    nodeStack[0] = start;
    edgeIdxStack[0] = 0;
    state[start] = 1;

    while (top >= 0) {
      const u = nodeStack[top];
      const edges = outAdj[u];
      const idx = edgeIdxStack[top];

      if (idx < edges.length) {
        edgeIdxStack[top] = idx + 1;
        const v = edges[idx];
        const vState = state[v];
        if (vState === 0) {
          state[v] = 1;
          top++;
          nodeStack[top] = v;
          edgeIdxStack[top] = 0;
        } else if (vState === 1) {
          edges[idx] = -1;
        }
      } else {
        state[u] = 2;
        top--;
      }
    }
  }

  for (let u = 0; u < nodeCount; u++) {
    const edges = outAdj[u];
    let write = 0;
    for (let read = 0; read < edges.length; read++) {
      if (edges[read] !== -1) {
        edges[write++] = edges[read];
      }
    }
    edges.length = write;
  }
}

function assignLayers(nodeCount: number, outAdj: number[][]): LayerAssignment {
  const inDegree = new Int32Array(nodeCount);
  for (let u = 0; u < nodeCount; u++) {
    const edges = outAdj[u];
    for (let i = 0; i < edges.length; i++) {
      inDegree[edges[i]]++;
    }
  }

  const queue = new Int32Array(nodeCount);
  let head = 0;
  let tail = 0;
  for (let u = 0; u < nodeCount; u++) {
    if (inDegree[u] === 0) {
      queue[tail++] = u;
    }
  }

  const layer = new Int32Array(nodeCount);
  let maxLayer = 0;
  while (head < tail) {
    const u = queue[head++];
    const nextLayer = layer[u] + 1;
    const edges = outAdj[u];
    for (let i = 0; i < edges.length; i++) {
      const v = edges[i];
      if (nextLayer > layer[v]) {
        layer[v] = nextLayer;
        if (nextLayer > maxLayer) {
          maxLayer = nextLayer;
        }
      }
      if (--inDegree[v] === 0) {
        queue[tail++] = v;
      }
    }
  }

  return { layer, layerCount: maxLayer + 1, complete: tail === nodeCount };
}

function buildInAdjacency(nodeCount: number, outAdj: number[][]): number[][] {
  const inAdj = createAdjacency(nodeCount);
  for (let u = 0; u < nodeCount; u++) {
    const edges = outAdj[u];
    for (let i = 0; i < edges.length; i++) {
      inAdj[edges[i]].push(u);
    }
  }
  return inAdj;
}

function reorderLayer(
  layerNodes: number[],
  neighbors: number[][],
  colOf: Int32Array,
  barySum: Float64Array,
  baryCount: Int32Array,
  movableNodes: number[],
  movableSlots: number[],
  compareByBarycenter: (a: number, b: number) => number,
): void {
  if (layerNodes.length <= 1) return;

  movableNodes.length = 0;
  movableSlots.length = 0;

  for (let slot = 0; slot < layerNodes.length; slot++) {
    const u = layerNodes[slot];
    const adj = neighbors[u];
    // Leave nodes without neighbors in this sweep direction in place so leaves do not drift.
    if (adj.length === 0) continue;

    let sum = 0;
    for (let i = 0; i < adj.length; i++) {
      sum += colOf[adj[i]];
    }
    barySum[u] = sum;
    baryCount[u] = adj.length;
    movableNodes.push(u);
    movableSlots.push(slot);
  }

  if (movableNodes.length <= 1) return;

  movableNodes.sort(compareByBarycenter);

  for (let i = 0; i < movableNodes.length; i++) {
    const u = movableNodes[i];
    const slot = movableSlots[i];
    layerNodes[slot] = u;
    colOf[u] = slot;
  }
}

function orderColumns(
  nodeCount: number,
  layerCount: number,
  layer: Int32Array,
  outAdj: number[][],
  inAdj: number[][],
): number[][] {
  const layers = createAdjacency(layerCount);
  const isolated: number[] = [];

  // Keep isolated nodes out of barycenter sweeps so adding one never perturbs connected cards.
  for (let u = 0; u < nodeCount; u++) {
    if (inAdj[u].length === 0 && outAdj[u].length === 0) {
      isolated.push(u);
    } else {
      layers[layer[u]].push(u);
    }
  }

  if (layerCount > 1) {
    const colOf = new Int32Array(nodeCount);
    for (let l = 0; l < layerCount; l++) {
      const row = layers[l];
      for (let col = 0; col < row.length; col++) {
        colOf[row[col]] = col;
      }
    }

    const barySum = new Float64Array(nodeCount);
    const baryCount = new Int32Array(nodeCount);
    const movableNodes: number[] = [];
    const movableSlots: number[] = [];
    // Cross-multiply to compare rational averages without floating-point rounding drift.
    const compareByBarycenter = (a: number, b: number): number => {
      const diff = barySum[a] * baryCount[b] - barySum[b] * baryCount[a];
      if (diff !== 0) return diff;
      return a - b;
    };

    for (let sweep = 0; sweep < BARYCENTER_SWEEPS; sweep++) {
      for (let l = 1; l < layerCount; l++) {
        reorderLayer(
          layers[l],
          inAdj,
          colOf,
          barySum,
          baryCount,
          movableNodes,
          movableSlots,
          compareByBarycenter,
        );
      }
      for (let l = layerCount - 2; l >= 0; l--) {
        reorderLayer(
          layers[l],
          outAdj,
          colOf,
          barySum,
          baryCount,
          movableNodes,
          movableSlots,
          compareByBarycenter,
        );
      }
    }
  }

  const rootLayer = layers[0];
  for (let i = 0; i < isolated.length; i++) {
    rootLayer.push(isolated[i]);
  }

  return layers;
}

export function layoutGraph(nodes: LayoutNode[], edges: LayoutEdge[]): Layout {
  if (nodes.length === 0) {
    return { placed: [], layers: 0, columns: 0 };
  }

  const { ordered, idToIndex } = canonicalizeNodes(nodes);
  const nodeCount = ordered.length;
  const outAdj = buildOutAdjacency(nodeCount, edges, idToIndex);

  let assignment = assignLayers(nodeCount, outAdj);
  if (!assignment.complete) {
    breakCycles(nodeCount, outAdj);
    assignment = assignLayers(nodeCount, outAdj);
  }

  const inAdj = buildInAdjacency(nodeCount, outAdj);
  const rows = orderColumns(nodeCount, assignment.layerCount, assignment.layer, outAdj, inAdj);

  const placed: Placed[] = new Array<Placed>(nodeCount);
  let write = 0;
  let columns = 0;
  for (let l = 0; l < rows.length; l++) {
    const row = rows[l];
    if (row.length > columns) {
      columns = row.length;
    }
    for (let col = 0; col < row.length; col++) {
      placed[write++] = { id: ordered[row[col]].id, layer: l, column: col };
    }
  }

  return { placed, layers: assignment.layerCount, columns };
}
