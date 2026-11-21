"use client";

import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import type { MemoryGraph } from "@/lib/types";
import { graphEdgeKey } from "../_lib/graph-edge-key";

/** The accessible alternative to the canvas: every link as a row. */
export function LinkTable({ graph, onOpen }: { graph: MemoryGraph; onOpen: (memoryId: string) => void }) {
  const titles = new Map(graph.nodes.map((node) => [node.memory_id, node.title]));
  const linked = new Set(graph.edges.flatMap((edge) => [edge.source_id, edge.target_id]));
  const alone = graph.nodes.filter((node) => !linked.has(node.memory_id));
  const open = (id: string, title: string) => (
    <button
      type="button"
      className="rounded-sm text-left underline-offset-4 outline-none hover:underline focus-visible:ring-3 focus-visible:ring-ring/50"
      onClick={() => onOpen(id)}
    >
      {title}
    </button>
  );
  return (
    <div className="max-h-80 overflow-auto rounded-lg border">
      <Table id="memory-links" aria-label="Memory links">
        <TableCaption className="sr-only">
          Directed links. Each row names the source memory, the relation, and the target memory.
        </TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead scope="col">Source</TableHead>
            <TableHead scope="col">Relation</TableHead>
            <TableHead scope="col">Target</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {graph.edges.map((edge) => (
            <TableRow key={graphEdgeKey(edge.source_id, edge.relation, edge.target_id)}>
              <TableCell className="whitespace-normal">{open(edge.source_id, titles.get(edge.source_id) ?? edge.source_id)}</TableCell>
              <TableCell>{edge.relation.replaceAll("_", " ")}</TableCell>
              <TableCell className="whitespace-normal">{open(edge.target_id, titles.get(edge.target_id) ?? edge.target_id)}</TableCell>
            </TableRow>
          ))}
          {alone.map((node) => (
            <TableRow key={`node:${node.memory_id}`}>
              <TableCell className="whitespace-normal">{open(node.memory_id, node.title)}</TableCell>
              <TableCell>no link</TableCell>
              <TableCell>—</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
