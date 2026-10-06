import { build } from "esbuild";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

// Render-only and injected-transport tests: no sign-in or network calls.
const result = await build({
  entryPoints: ["tests/chatgpt-cleanup-ui.test.tsx"],
  bundle: true,
  platform: "node",
  format: "cjs",
  packages: "external",
  write: false,
});
// Windows has a small process-command length limit. Run a file instead of
// passing the full bundle through `node -e`. Keep it below the repo root so
// external dependencies resolve through this checkout's node_modules.
const testDirectory = await mkdtemp(join(process.cwd(), ".chatgpt-ui-test-"));
try {
  const testPath = join(testDirectory, "test.cjs");
  await writeFile(testPath, result.outputFiles[0].text);
  const test = spawnSync(process.execPath, [testPath], { stdio: "inherit" });
  if (test.error) throw test.error;
  process.exitCode = test.status ?? 1;
} finally {
  await rm(testDirectory, { recursive: true, force: true });
}
