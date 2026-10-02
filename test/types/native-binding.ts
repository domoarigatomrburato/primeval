// Drift check between the hand-written native types and the `binding.d.ts`
// that `npm run build:node` generates. Type-checked by test/types.test.js.
import type * as Generated from "../../binding.js";
import type * as Handwritten from "../../src/native-binding.js";

type Mutual<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;

export const nativeTypesMatch: [
  Mutual<Generated.NativeApproximateRequest, Handwritten.NativeApproximateRequest>,
  Mutual<Generated.NativeRenderOptions, Handwritten.NativeRenderOptions>,
  Mutual<Generated.NativeExecutionOptions, Handwritten.NativeExecutionOptions>,
  Mutual<Generated.NativeProgressInfo, Handwritten.NativeProgressInfo>,
  Mutual<Generated.NativeApproximateResult, Handwritten.NativeApproximateResult>,
  Mutual<Generated.NativeTask, Handwritten.NativeTask>,
  Mutual<typeof Generated.startApproximate, Handwritten.NativeBinding["startApproximate"]>,
] = [true, true, true, true, true, true, true];
