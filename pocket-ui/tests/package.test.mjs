import test from "node:test";
import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";

const projectRoot = resolve(import.meta.dirname, "..");
const packagePath = resolve(projectRoot, "dist/timeline-probe.pocket");

test("Vita package contains the native Solid guest, live feed callback, and packaged image asset", async () => {
  const result = spawnSync(
    "bun",
    [
      resolve(projectRoot, "../vendor/pocketjs/tools/pocket-pack.ts"),
      "build",
      "--manifest",
      resolve(projectRoot, "pocket.json"),
      "--project-root",
      projectRoot,
      "--target",
      "vita",
      "-o",
      packagePath,
    ],
    { cwd: projectRoot, encoding: "utf8", timeout: 120_000 },
  );
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);

  const manifest = JSON.parse(
    await readFile(resolve(projectRoot, "pocket.json"), "utf8"),
  );
  assert.equal(manifest.app.framework, "solid");
  assert.equal("aot" in manifest.app, false);
  assert.equal("model" in manifest.app, false);

  const packageStat = await stat(packagePath);
  assert.ok(packageStat.size > 0, "packager emitted a non-empty .pocket file");
  assert.match(result.stdout, /variant vita: js \d+B pak [1-9]\d*B/);
  assert.match(result.stdout, /variant vita: js [1-9]\d*B pak [1-9]\d*B/);
  const packagedGuest = await readFile(packagePath);
  assert.ok(packagedGuest.includes(Buffer.from("__bskyReceiveFeed")), "guest includes the native feed receive callback");
  assert.ok(packagedGuest.includes(Buffer.from("Following")), "guest contains the real timeline view");
});
