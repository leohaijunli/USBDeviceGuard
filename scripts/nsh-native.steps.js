// Feed to `nsh test nsh.json --target windows|linux` from devguard-gui/.
// These records are produced by UI actions and real local background threads.
await app.setViewport(1120, 800);
await app.waitFor("devguard");
if (app.shows("role:button[Light mode]")) {
  await app.press("role:button[Light mode]");
}
await app.waitFor("USB serial devices");
try {
  // The watcher populates the list within a couple of seconds on this machine.
  await app.waitFor("text:COM26", { timeout: 8000 });
} catch {
  // Hardware may be unplugged; keep the empty-list screenshot as evidence.
}
await app.screenshot("../artifacts/nsh-spike-devices.png");
if (app.shows("role:button[V6X Rust CDC]")) {
  await app.press("role:button[V6X Rust CDC]");
  await app.waitFor("Copy details");
  await app.screenshot("../artifacts/nsh-spike-device-details.png");
}
await app.press("role:tab[Records]");
await app.press("role:button[Send event]");
await app.waitFor("1 local record");
await app.press("role:button[Add record…]");
await app.type("id:record-title", "IME and note validation");
await app.type("id:record-note", "Record from a local thread.\nUsed to check the form, search and the details pane.");
await app.press("role:button[Send record]");
await app.waitFor("2 local records");
await app.press("role:button[Send event]");
await app.waitFor("3 local records");
await app.press("role:button[Add record…]");
await app.type("id:record-title", "Draft dismissed with Escape");
await app.keyboard.press("escape");
if (app.shows("id:record-title")) {
  // Observed on Windows 0.4.0: Escape while the dialog's input has focus does
  // not dismiss the modal. Cancel is the guaranteed path; retest on Linux.
  await app.press("role:button[Cancel]");
}
await app.waitFor("id:record-title", { state: "gone" });
await app.press("role:button[IME and note validation]");
await app.waitFor("Cross-thread delivery verified");
await app.screenshot("../artifacts/nsh-spike-light.png");

await app.press("role:button[Dark mode]");
await app.waitFor("role:button[Light mode]");
await app.screenshot("../artifacts/nsh-spike-dark.png");
await app.press("role:button[Pause preview…]");
await app.press("role:radio[Whole service]");
await app.press("role:radio[15 min]");
await app.screenshot("../artifacts/nsh-spike-dialog.png");
await app.press("role:button[Start preview]");
await app.waitFor("role:button[End preview]");
await app.screenshot("../artifacts/nsh-spike-paused.png");
await app.press("role:button[End preview]");

await app.setViewport(640, 800);
if (app.shows("role:button[Back to list]")) {
  // The wide-screen steps selected a record; the narrow layout shows one pane.
  await app.press("role:button[Back to list]");
}
await app.press("role:button[IME and note validation]");
await app.waitFor("role:button[Back to list]");
await app.screenshot("../artifacts/nsh-spike-narrow.png");
await app.press("role:button[Back to list]");
await app.waitFor("role:button[IME and note validation]");
