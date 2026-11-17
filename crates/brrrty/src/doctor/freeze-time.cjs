// Preload for `node --require`: freezes the clock at SOURCE_DATE_EPOCH, the way
// Brrrty reads it, so a build that prints the current time is reproducible.
//
// `Date.now()`, `new Date()` and `Date()` with no arguments return the epoch.
// Dates built from a value, such as `new Date(stat.mtimeMs)`, are unchanged,
// so file dates still come from the file system. Without SOURCE_DATE_EPOCH
// this file does nothing.
"use strict";

const raw = process.env.SOURCE_DATE_EPOCH;
if (raw !== undefined) {
  if (!/^[0-9]+$/.test(raw)) {
    throw new Error(`SOURCE_DATE_EPOCH must be whole seconds, got ${JSON.stringify(raw)}`);
  }
  const frozen = Number(raw) * 1000;
  const RealDate = Date;
  function FrozenDate(...args) {
    if (new.target === undefined) {
      return new RealDate(frozen).toString();
    }
    return Reflect.construct(RealDate, args.length === 0 ? [frozen] : args, new.target);
  }
  Object.setPrototypeOf(FrozenDate, RealDate);
  Object.defineProperty(FrozenDate, "prototype", { value: RealDate.prototype });
  Object.defineProperty(FrozenDate, "name", { value: "Date" });
  FrozenDate.now = function now() {
    return frozen;
  };
  globalThis.Date = FrozenDate;
}
