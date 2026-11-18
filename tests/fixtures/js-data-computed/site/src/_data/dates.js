// luxon reads the clock inside `fromISO` (for a zone's offset); the dates
// it gives here are fixed.
const { DateTime } = require("luxon");
module.exports = {
  long: (iso) => DateTime.fromISO(iso).toFormat("LLLL d, yyyy"),
  zoned: (iso) => DateTime.fromISO(iso, { zone: "America/Chicago" }).toISO(),
};
