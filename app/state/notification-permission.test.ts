import { beforeEach, describe, expect, it } from "vitest";

import {
  notificationPermission,
  sendTestNotification,
} from "./notification-permission";

describe("sendTestNotification", () => {
  beforeEach(() => {
    notificationPermission.value = "unknown";
  });

  it("asks for permission BEFORE sending, and puts the new status on the card", async () => {
    const calls: string[] = [];
    const r = await sendTestNotification({
      notificationRequestPermission: async () => {
        calls.push("request");
        return "granted";
      },
      notificationSendTest: async () => {
        // The card already shows the answer when the notification goes out.
        calls.push(`send:${notificationPermission.value}`);
        return true;
      },
    });
    expect(calls).toEqual(["request", "send:granted"]);
    expect(r).toEqual({ sent: true, permission: "granted" });
    expect(notificationPermission.value).toBe("granted");
  });

  it("still sends when the answer was denied, and reports it", async () => {
    const r = await sendTestNotification({
      notificationRequestPermission: async () => "denied",
      notificationSendTest: async () => true,
    });
    expect(r.permission).toBe("denied");
    expect(notificationPermission.value).toBe("denied");
  });

  it("reports a failed send", async () => {
    const r = await sendTestNotification({
      notificationRequestPermission: async () => "unknown",
      notificationSendTest: async () => false,
    });
    expect(r.sent).toBe(false);
  });
});
