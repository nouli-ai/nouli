import type { Network } from "./networks";
import { support } from "./teams/support";
import { finance } from "./teams/finance";
import { sales } from "./teams/sales";
import { operations } from "./teams/operations";
import { compliance } from "./teams/compliance";

/**
 * Invented networks for the team picker, each a different shape: questions that read earlier
 * answers, facts that join late, checks feeding scores, and forks into more than one output.
 */
export const teamNetworks: readonly Network[] = [support, finance, sales, operations, compliance];
