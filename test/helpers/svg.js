import assert from "node:assert/strict";

/** The shape lines of an SVG document: those between the background and `</svg>`. */
export function svgShapeLines(svg) {
  const lines = svg.trimEnd().split("\n");
  assert.match(lines[1], /^<rect width=/);
  assert.equal(lines.at(-1), "</svg>");
  return lines.slice(2, -1);
}
