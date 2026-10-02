import assert from "node:assert/strict";

/** The shape lines of an SVG document: those between the background and `</svg>`. */
export function svgShapeLines(svg) {
  const lines = svg.trimEnd().split("\n");
  assert.match(lines[1], /^<rect width=/);
  assert.equal(lines.at(-1), "</svg>");
  return lines.slice(2, -1);
}

/**
 * The element name of an SVG shape line. A circle is an ellipse whose radii
 * are equal, and the final refit pass can make them equal or unequal, so the
 * two are one element kind here.
 */
export function svgElementName(line) {
  const match = /^<([a-z]+)[\s/>]/.exec(line);
  assert.ok(match, `not an element: ${line}`);
  return match[1] === "circle" ? "ellipse" : match[1];
}

/**
 * Asserts the progress contract: one single-line shape per step, previewing
 * the final SVG, whose shape lines have the same number and, in order, the
 * same element kinds; the final refit pass may revise their attributes.
 */
export function assertPreviewsSvg(shapes, svg, steps) {
  assert.equal(shapes.length, steps);
  assert.ok(shapes.every((shape) => typeof shape === "string" && !shape.includes("\n")));
  const lines = svgShapeLines(svg);
  assert.equal(lines.length, shapes.length);
  assert.deepEqual(lines.map(svgElementName), shapes.map(svgElementName));
}
