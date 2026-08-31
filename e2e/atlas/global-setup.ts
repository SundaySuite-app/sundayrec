import { resetRecords } from "./report";

/** Start every atlas run from an empty scene log — see e2e/atlas/report.ts. */
export default function globalSetup(): void {
  resetRecords();
}
