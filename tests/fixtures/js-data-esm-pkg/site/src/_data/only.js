export default function (arg) {
  return { called: true, hasEleventy: "eleventy" in arg, thisType: typeof this };
}
