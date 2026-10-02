import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";

import {
  artifactChecksForTarget,
  findGlibcViolations,
  findIsaViolations,
} from "../../scripts/check-artifact.mjs";

const repoRoot = process.cwd();

// Excerpts in the shape of `objdump -d --no-show-raw-insn` output (GNU and LLVM).
const X86_PORTABLE = `
primeval-node.linux-x64-gnu.node:	file format elf64-x86-64

Disassembly of section .text:

000000000000e000 <_ZN4core3fmt5write17hzmm1k1vpternlogE>:
    e000:      	subq	$0x8, %rsp
    e004:      	vmovdqu	(%rdi), %ymm0
    e008:      	vpaddd	%xmm1, %xmm2, %xmm3
    e00c:      	callq	0xe1e8 <vpternlog_zmm0@plt>
    e010:      	movq	0xfedbd(%rip), %rax     # 0x10cdc8 <zmm2>
    e014:      	testq	%rax, %rax
`;

const X86_AVX512_ATT = `
    e000:      	vpternlogd	$0x96, %zmm1, %zmm2, %zmm3
    e004:      	vmovdqu8	(%rdi), %ymm0 {%k1} {z}
`;

const X86_AVX512_INTEL = `
    e000:      	vmovdqu64	zmm0, zmmword ptr [rdi]
    e004:      	vpaddd	ymm0 {k2}, ymm1, ymm2
`;

const ARM_PORTABLE = `
primeval-node.linux-arm64-gnu.node:	file format elf64-littleaarch64

0000000000016000 <_ZN5ptrue7whilelo17hz0E>:
   16004:      	orr	v1.16b, v2.16b, v1.16b
   16008:      	uaddl2	v0.8h, v0.16b, v2.16b
   1600c:      	swpal	xzr, x0, [x8]
   16010:      	ldr	q0, [x0, #0x10]
   16014:      	bl	0x17000 <whilelo_z0@plt>
   16018:      	fmov	s0, wzr  // z0.s
`;

const ARM_SVE = `
   16000:      	ptrue	p0.s
   16004:      	ld1w	{ z0.s }, p0/z, [x0]
   16008:      	whilelo	p1.s, x8, x9
   1600c:      	mov	z1.d, z0.d
`;

test("x86_64 ISA check accepts SSE/AVX2 code and ignores symbol names and comments", () => {
  assert.deepEqual(findIsaViolations(X86_PORTABLE, "x86_64"), []);
});

test("x86_64 ISA check rejects AVX-512 registers, masks and vpternlog in AT&T syntax", () => {
  const violations = findIsaViolations(X86_AVX512_ATT, "x86_64");
  assert.deepEqual(
    violations.map(({ line, reason }) => [line, reason]),
    [
      [2, "AVX-512 zmm register"],
      [3, "AVX-512 mask register"],
    ],
  );
  assert.match(violations[0].instruction, /vpternlogd/);
});

test("x86_64 ISA check rejects AVX-512 in Intel syntax", () => {
  assert.deepEqual(
    findIsaViolations(X86_AVX512_INTEL, "x86_64").map(({ reason }) => reason),
    ["AVX-512 zmm register", "AVX-512 mask register"],
  );
});

test("x86_64 ISA check flags vpternlog even without a zmm operand", () => {
  assert.deepEqual(
    findIsaViolations("    e000:\tvpternlogq\t$0xca, %ymm1, %ymm2, %ymm3\n", "x86_64").map(
      ({ reason }) => reason,
    ),
    ["AVX-512 vpternlog instruction"],
  );
});

test("aarch64 ISA check accepts NEON, LSE atomics and zero registers", () => {
  assert.deepEqual(findIsaViolations(ARM_PORTABLE, "aarch64"), []);
});

test("aarch64 ISA check rejects SVE registers and instructions", () => {
  assert.deepEqual(
    findIsaViolations(ARM_SVE, "aarch64").map(({ line, reason }) => [line, reason]),
    [
      [2, "SVE instruction"],
      [3, "SVE vector register"],
      [4, "SVE instruction"],
      [5, "SVE vector register"],
    ],
  );
});

test("ISA check rejects an unknown architecture", () => {
  assert.throws(() => findIsaViolations("", "riscv64"), /unsupported architecture/);
});

test("glibc check accepts symbol versions up to the maximum", () => {
  const dynamicSymbols = `
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.2.5) getenv
0000000000000000      DF *UND*	0000000000000000  GLIBC_2.14  memcpy
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.17) clock_gettime
0000000000000000      DO *UND*	0000000000000000 (GLIBC_PRIVATE) _rtld_global
0000000000000000  w   D  *UND*	0000000000000000  __gmon_start__
`;
  assert.deepEqual(findGlibcViolations(dynamicSymbols, "2.17"), []);
});

test("glibc check rejects symbol versions above the maximum", () => {
  const dynamicSymbols = `
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.2.5) getenv
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.34) __libc_start_main
0000000000000000      DF *UND*	0000000000000000  GLIBC_2.18  __cxa_thread_atexit_impl
0000000000000000      DF *UND*	0000000000000000 (GLIBC_2.17.1) hypothetical
`;
  assert.deepEqual(findGlibcViolations(dynamicSymbols, "2.17"), [
    { version: "2.34", symbol: "__libc_start_main" },
    { version: "2.18", symbol: "__cxa_thread_atexit_impl" },
    { version: "2.17.1", symbol: "hypothetical" },
  ]);
});

test("artifact checks are derived from the napi target", () => {
  assert.deepEqual(artifactChecksForTarget("x86_64-unknown-linux-gnu"), {
    arch: "x86_64",
    maxGlibc: "2.17",
  });
  assert.deepEqual(artifactChecksForTarget("aarch64-unknown-linux-gnu"), {
    arch: "aarch64",
    maxGlibc: "2.17",
  });
  assert.deepEqual(artifactChecksForTarget("x86_64-pc-windows-msvc"), {
    arch: "x86_64",
    maxGlibc: null,
  });
  assert.deepEqual(artifactChecksForTarget("aarch64-apple-darwin"), {
    arch: "aarch64",
    maxGlibc: null,
  });
  assert.deepEqual(artifactChecksForTarget("x86_64-apple-darwin"), {
    arch: "x86_64",
    maxGlibc: null,
  });
  assert.throws(() => artifactChecksForTarget("riscv64gc-unknown-linux-gnu"), /unsupported/);
});

test("every napi target has artifact checks", () => {
  const pkg = JSON.parse(fs.readFileSync(path.join(repoRoot, "package.json"), "utf8"));
  for (const target of pkg.napi.targets) {
    assert.doesNotThrow(() => artifactChecksForTarget(target), target);
  }
});

test("no committed cargo config sets target-cpu", () => {
  // Tracked and not-ignored files, so a config that is about to be committed counts too.
  const configs = execFileSync(
    "git",
    ["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    { cwd: repoRoot, encoding: "utf8" },
  )
    .split("\0")
    .filter((file) => /(^|\/)\.cargo\/config(\.toml)?$/.test(file))
    .filter((file) => fs.existsSync(path.join(repoRoot, file)));

  for (const file of configs) {
    const source = fs.readFileSync(path.join(repoRoot, file), "utf8");
    assert.doesNotMatch(source, /target-cpu/, `${file} sets target-cpu`);
  }
});

test("release builds never set target-cpu", () => {
  const sources = [
    "package.json",
    ...fs
      .readdirSync(path.join(repoRoot, ".github", "workflows"))
      .map((name) => path.join(".github", "workflows", name)),
  ];
  for (const file of sources) {
    const source = fs.readFileSync(path.join(repoRoot, file), "utf8");
    assert.doesNotMatch(source, /target-cpu/, `${file} sets target-cpu`);
  }
});
