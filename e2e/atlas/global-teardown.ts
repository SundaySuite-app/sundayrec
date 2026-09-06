import { writeReports } from "./report";

/** Assemble INDEX.md + CONSOLE-FINDINGS.md once, after every scene has been
 *  photographed — including the ones whose worker was restarted after a
 *  failure. See the header of e2e/atlas/report.ts for why this is not an
 *  `afterAll`. */
export default function globalTeardown(): void {
  writeReports();
}
