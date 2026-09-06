// The renderer half of the error-code seam (R3-C).
//
// Rust leads every branchable error message with a stable snake code
// (`"<category>: <code>[: prose]"`); `errorCode()` is the ONE extractor call
// sites branch on. These cases pin the exact wire strings the Rust side
// produces today — including the `no_config_smtp_host` seam whose Rust half is
// pinned by `commands/email.rs::the_smtp_host_error_leads_with_the_stable_code`.

import { describe, expect, it } from "vitest";
import { errorCode } from "./error-code-core";

describe("errorCode", () => {
  it("extracts the code from the {code, message} rejection shape", () => {
    // What a rejected invoke actually delivers (error.rs Serialize impl).
    expect(
      errorCode({
        code: "validation",
        message: "validation: no_save_folder: nothing configured",
      }),
    ).toBe("no_save_folder");
  });

  it("strips every AppError category prefix", () => {
    expect(errorCode("validation: cancelled")).toBe("cancelled");
    expect(errorCode("recording error: timeout")).toBe("timeout");
    expect(errorCode("io error: disk_full")).toBe("disk_full");
    expect(errorCode("internal: feature_disabled: streaming requires …")).toBe(
      "feature_disabled",
    );
  });

  it("matches the exact smtp-host wire message (the general-page seam)", () => {
    // The Rust side (commands/email.rs) pins the same literal; if either half
    // moves without the other, one of the two tests fails.
    expect(
      errorCode("validation: no_config_smtp_host: smtp host missing"),
    ).toBe("no_config_smtp_host");
    // The OTHER no_config variants still resolve to their shared prefix code.
    expect(errorCode("validation: no_config: smtp from")).toBe("no_config");
    expect(errorCode("validation: no_config: save folder not set")).toBe(
      "no_config",
    );
  });

  it("handles bare codes and already-extracted error strings", () => {
    expect(errorCode("cancelled")).toBe("cancelled");
    expect(errorCode({ error: "feature_disabled: not in this build" })).toBe(
      "feature_disabled",
    );
    expect(errorCode(new Error("validation: empty_file"))).toBe("empty_file");
  });

  it('returns "" for prose that carries no code — never a half-word', () => {
    expect(errorCode("Could not open the device")).toBe("");
    expect(errorCode("")).toBe("");
    expect(errorCode(null)).toBe("");
    expect(errorCode(undefined)).toBe("");
    // A word followed by more letters/digits is still a clean token boundary
    // decision: `no_config2x` is NOT the `no_config` code.
    expect(errorCode("validation: no_configX")).toBe("");
  });
});
