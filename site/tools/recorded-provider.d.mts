import type { Server } from "node:http";
import type { GraphNode } from "../src/lib/schemas/graph";
export function startStub(recorded: { node: { questions: GraphNode["questions"] }; used: boolean }[], options?: { tui?: boolean }): Promise<{server: Server; failures: string[]; port: number}>;
