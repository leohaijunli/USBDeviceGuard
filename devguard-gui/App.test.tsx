import { expect, open, test } from "nsh/test";

test("a background event updates the real Rust UI", async () => {
  const app = await open(".", { width: 1120, height: 800 });
  await app.press("role:tab[Records]");
  await app.press("role:button[Send event]");
  await app.waitFor("Cross-thread delivery verified");
  expect(app.texts()).toContain("1 local record");
});

test("the devices tab stays read-only", async () => {
  const app = await open(".", { width: 1120, height: 800, platform: "linux" });
  await app.waitFor("USB serial devices");
  expect(app.texts()).toContain("Read-only inventory. Permissions, baud rates and blocking are applied by devguardd on Linux.");
});

test("the pause dialog can be cancelled", async () => {
  const app = await open(".", { width: 1120, height: 800 });
  await app.press("role:button[Pause preview…]");
  await app.press("role:radio[30 min]");
  await app.press("role:button[Cancel]");
  expect(app.texts()).not.toContain("End preview");
});
