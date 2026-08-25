import { execSync } from "node:child_process";
import { copyFileSync, mkdirSync, existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";

// Copies the built cleanup-engine binary to the location Tauri's
// `externalBin` expects: src-tauri/binaries/cleanup-engine-<target-triple>[.exe]
//
// Tauri's BUILD SCRIPT validates that this file exists whenever the crate
// compiles (not just at bundle time), so this script must run BEFORE any
// `cargo build`/`cargo test` on a fresh checkout. When the engine hasn't
// been compiled yet we drop in a zero-byte placeholder purely to satisfy
// that validation; build pipelines run this script again AFTER the engine
// build so the real binary always replaces the placeholder before bundling.
//
// Target triple resolution order:
//   1. first CLI arg (used by CI cross-compile jobs)
//   2. TAURI_ENV_TARGET_TRIPLE env (set inside `tauri build`)
//   3. `rustc -vV` host triple

const argTriple = process.argv[2];
const triple =
  argTriple ||
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  execSync("rustc -vV", { encoding: "utf8" }).match(/host: (\S+)/)[1];
const ext = triple.includes("windows") ? ".exe" : "";

// Cross-compiles (`--target <triple>`) output under target/<triple>/release;
// host builds land in target/release.
const candidates = [
  join("src-tauri", "target", triple, "release", `cleanup-engine${ext}`),
  join("src-tauri", "target", "release", `cleanup-engine${ext}`),
];
const src = candidates.find((p) => existsSync(p));

const dstDir = join("src-tauri", "binaries");
mkdirSync(dstDir, { recursive: true });
const dst = join(dstDir, `cleanup-engine-${triple}${ext}`);

if (src) {
  copyFileSync(src, dst);
  console.log(`copied ${src} -> ${dst}`);
} else if (existsSync(dst)) {
  console.log(`engine not rebuilt; reusing existing ${dst}`);
} else {
  writeFileSync(dst, "");
  console.warn(
    `WARNING: wrote PLACEHOLDER ${dst} (engine not built yet).\n` +
      `It satisfies tauri-build's externalBin check for compile/test only —\n` +
      `a real engine build must replace it before bundling or the app will\n` +
      `ship a broken helper.`,
  );
}
