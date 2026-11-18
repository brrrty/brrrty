// The keys a computed data function reads: Eleventy's ComputedDataProxy,
// ported in a later step. Until then, asking is a protocol error.
async function varsUsed() {
  throw new Error("varsUsed is not supported by this worker yet");
}
