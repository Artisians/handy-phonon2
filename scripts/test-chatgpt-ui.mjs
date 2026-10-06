import { build } from "esbuild";
import { spawnSync } from "node:child_process";

// Render-only and injected-transport tests: no sign-in or network calls.
const result = await build({
  entryPoints: ["tests/chatgpt-cleanup-ui.test.tsx"],
  bundle: true,
  platform: "node",
  format: "cjs",
  packages: "external",
  write: false,
});
const test = spawnSync(process.execPath, ["-e", result.outputFiles[0].text], {
  stdio: "inherit",
});
process.exitCode = test.status ?? 1;
