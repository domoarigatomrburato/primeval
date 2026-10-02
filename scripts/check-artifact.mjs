// Release gate for one native artifact: the disassembly must not need CPU
// features beyond the target's baseline (no AVX-512 on x86_64, no SVE on
// aarch64), and Linux GNU artifacts must not need a glibc newer than 2.17.
//
// Usage: node scripts/check-artifact.mjs <napi target> [file.node]
// (default: ./<napi.binaryName>.<suffix>.node, as `napi build --platform` writes it)
//
// Uses `llvm-objdump` from the pinned Rust toolchain (`rustup component add
// llvm-tools`), which reads ELF, Mach-O and COFF the same way on every runner.
// Set OBJDUMP to use another objdump.
import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";

import { readPackageMetadata, runtimeTargetForTarget } from "./napi-targets.mjs";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** Oldest glibc the Linux GNU prebuilds support (the napi-rs cross toolchain sysroot). */
export const MAX_GLIBC = "2.17";

const ISA_ARCH = { x64: "x86_64", arm64: "aarch64" };

// One reason per instruction line; the first matching rule wins.
const ISA_RULES = {
  x86_64: [
    { reason: "AVX-512 zmm register", pattern: /\bzmm\d+\b/ },
    { reason: "AVX-512 mask register", pattern: /\bk[0-7]\b/ },
    { reason: "AVX-512 vpternlog instruction", pattern: /^vpternlog[dq]\b/ },
  ],
  aarch64: [
    {
      reason: "SVE instruction",
      pattern: /^(?:ptrues?|whilel[eost]|whileh[is]|rdvl|addvl|addpl)\b/,
    },
    { reason: "SVE vector register", pattern: /\bz(?:[12]?\d|3[01])(?:\.[bhsdq])?\b/ },
    { reason: "SVE predicate register", pattern: /\bp(?:1[0-5]|\d)(?:\/[zm]|\.[bhsdq])/ },
  ],
};

/** The checks for one napi target: its ISA family and, for Linux GNU, the newest allowed glibc. */
export function artifactChecksForTarget(target) {
  const { arch, abi } = runtimeTargetForTarget(target);
  if (!ISA_ARCH[arch]) {
    throw new Error(`unsupported architecture for artifact checks: ${arch}`);
  }
  return { arch: ISA_ARCH[arch], maxGlibc: abi === "gnu" ? MAX_GLIBC : null };
}

/**
 * Returns the instruction text of one `objdump -d --no-show-raw-insn` line,
 * without address, symbol references (`<...>`) or trailing comments, or null
 * when the line is not an instruction.
 */
export function instructionText(line) {
  const match = line.match(/^\s*[0-9a-f]+:\s+(.*)$/i);
  if (!match) {
    return null;
  }
  return (
    match[1]
      .replace(/<[^>]*>/g, "")
      // x86 comments are "# ...", Arm comments "// ..."; Arm immediates ("#0x10") stay.
      .replace(/\s(?:#\s|\/\/).*$/, "")
      .trim()
  );
}

/**
 * Finds instructions that need CPU features outside the portable baseline.
 * `disassembly` is `objdump -d --no-show-raw-insn` output (a string or an
 * iterable of lines).
 */
export function findIsaViolations(disassembly, arch) {
  const rules = ISA_RULES[arch];
  if (!rules) {
    throw new Error(`unsupported architecture for ISA checks: ${arch}`);
  }

  const lines = typeof disassembly === "string" ? disassembly.split("\n") : disassembly;
  const violations = [];
  let lineNumber = 0;
  for (const line of lines) {
    lineNumber += 1;
    const instruction = instructionText(line);
    if (!instruction) {
      continue;
    }
    const rule = rules.find(({ pattern }) => pattern.test(instruction));
    if (rule) {
      violations.push({ line: lineNumber, reason: rule.reason, instruction });
    }
  }
  return violations;
}

function compareVersions(left, right) {
  const a = left.split(".").map(Number);
  const b = right.split(".").map(Number);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0) - (b[index] ?? 0);
    if (difference !== 0) {
      return difference;
    }
  }
  return 0;
}

/**
 * Finds dynamic symbols that need a glibc newer than `maxVersion`.
 * `dynamicSymbols` is `objdump -T` output.
 */
export function findGlibcViolations(dynamicSymbols, maxVersion) {
  const violations = [];
  for (const line of dynamicSymbols.split("\n")) {
    const match = line.match(/\(?GLIBC_(\d+(?:\.\d+)+)\)?\s+(\S+)\s*$/);
    if (match && compareVersions(match[1], maxVersion) > 0) {
      violations.push({ version: match[1], symbol: match[2] });
    }
  }
  return violations;
}

function objdumpCommand() {
  if (process.env.OBJDUMP) {
    return process.env.OBJDUMP;
  }
  const sysroot = execFileSync("rustc", ["--print", "sysroot"], { encoding: "utf8" }).trim();
  const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (\S+)$/m)?.[1];
  const executable = process.platform === "win32" ? "llvm-objdump.exe" : "llvm-objdump";
  const candidate = path.join(sysroot, "lib", "rustlib", host ?? "", "bin", executable);
  if (!fs.existsSync(candidate)) {
    throw new Error(
      `llvm-objdump not found at ${candidate}; run \`rustup component add llvm-tools\` or set OBJDUMP`,
    );
  }
  return candidate;
}

async function disassemblyViolations(objdump, file, arch) {
  const child = spawn(objdump, ["-d", "--no-show-raw-insn", file], {
    stdio: ["ignore", "pipe", "inherit"],
  });
  const exited = new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", resolve);
  });
  const lines = readline.createInterface({ input: child.stdout, crlfDelay: Infinity });
  const violations = [];
  let lineNumber = 0;
  for await (const line of lines) {
    lineNumber += 1;
    for (const violation of findIsaViolations([line], arch)) {
      violations.push({ ...violation, line: lineNumber });
    }
  }
  const status = await exited;
  if (status !== 0) {
    throw new Error(`${objdump} -d exited with status ${status}`);
  }
  if (lineNumber === 0) {
    throw new Error(`${objdump} -d produced no output`);
  }
  return violations;
}

function summarize(violations, describe) {
  const shown = violations.slice(0, 20).map((violation) => `  ${describe(violation)}`);
  if (violations.length > shown.length) {
    shown.push(`  ... and ${violations.length - shown.length} more`);
  }
  return shown.join("\n");
}

async function main(argv) {
  const [target, explicitFile] = argv;
  if (!target) {
    throw new Error("usage: node scripts/check-artifact.mjs <napi target> [file.node]");
  }
  const { binaryName } = readPackageMetadata().napi;
  const file =
    explicitFile ??
    path.join(REPO_ROOT, `${binaryName}.${runtimeTargetForTarget(target).suffix}.node`);
  if (!fs.existsSync(file)) {
    throw new Error(`artifact does not exist: ${file}`);
  }

  const { arch, maxGlibc } = artifactChecksForTarget(target);
  const objdump = objdumpCommand();
  let failed = false;

  const isaViolations = await disassemblyViolations(objdump, file, arch);
  if (isaViolations.length > 0) {
    failed = true;
    console.error(
      `${file}: ${isaViolations.length} instructions outside the ${arch} baseline:\n${summarize(
        isaViolations,
        ({ line, reason, instruction }) => `line ${line}: ${reason}: ${instruction}`,
      )}`,
    );
  } else {
    console.log(`${file}: no instructions outside the ${arch} baseline`);
  }

  if (maxGlibc !== null) {
    const dynamicSymbols = execFileSync(objdump, ["-T", file], {
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    });
    if (!/GLIBC_\d/.test(dynamicSymbols)) {
      throw new Error(`${file}: no versioned glibc symbols found; is this a Linux GNU artifact?`);
    }
    const glibcViolations = findGlibcViolations(dynamicSymbols, maxGlibc);
    if (glibcViolations.length > 0) {
      failed = true;
      console.error(
        `${file}: ${glibcViolations.length} symbols need glibc newer than ${maxGlibc}:\n${summarize(
          glibcViolations,
          ({ version, symbol }) => `${symbol} (GLIBC_${version})`,
        )}`,
      );
    } else {
      console.log(`${file}: every glibc symbol version is at most ${maxGlibc}`);
    }
  }

  if (failed) {
    process.exitCode = 1;
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
