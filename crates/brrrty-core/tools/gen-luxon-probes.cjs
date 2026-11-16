// Regenerates tests/probes/luxon.json: Luxon 3.7.2's
// DateTime.fromJSDate(new Date(ms), { zone }).toFormat(fmt), for the tokens,
// zones and dates below. Run from crates/brrrty-core with Node 20 or later
// (the file was made with v26.2.0):
//
//   LUXON=/path/to/node_modules/luxon node tools/gen-luxon-probes.cjs > tests/probes/luxon.json
//
// LUXON is the luxon package directory (version 3.7.2). Output is JSON:
// { "luxon": "3.7.2", "cases": [{ "ms": n | null, "zone": s, "fmt": s, "out": s }] }.
// "ms" null is an Invalid Date. Short zone names (ZZZZ) come from this
// Node's ICU, so regenerate zones.rs too when ICU changes.
const { DateTime } = require(process.env.LUXON || (() => { throw new Error('set LUXON to the luxon package directory'); })());

const utc = (y, mo, d, h = 0, mi = 0, s = 0, ms = 0) => {
  const t = new Date(0);
  t.setUTCFullYear(y, mo - 1, d);
  t.setUTCHours(h, mi, s, ms);
  return t.getTime();
};

const DATES = [
  utc(2026, 11, 16, 15, 45, 12, 345), // an afternoon
  utc(2026, 1, 1, 0, 0, 0, 0), // midnight, new year
  utc(2024, 2, 29, 12, 0, 0, 7), // leap day
  utc(2023, 12, 31, 23, 59, 59, 999), // end of a year (ISO week 52, week year differs)
  utc(2021, 1, 3, 9, 5, 3, 50), // a Sunday that belongs to ISO week 53 of 2020
  utc(2020, 12, 31, 12, 0, 0, 0), // ISO week 53
  utc(2025, 12, 29, 6, 30, 0, 0), // ISO week 1 of 2026
  utc(2024, 3, 10, 6, 59, 59, 0), // just before US DST starts
  utc(2024, 3, 10, 7, 0, 0, 0), // US DST starts
  utc(2024, 11, 3, 5, 30, 0, 0), // US DST ends (first 1:30)
  utc(2024, 11, 3, 6, 30, 0, 0), // US DST ends (second 1:30)
  utc(2024, 3, 31, 0, 59, 0, 0), // EU DST starts
  utc(2024, 3, 31, 1, 0, 0, 0),
  utc(2024, 10, 6, 15, 0, 0, 0), // Lord Howe DST (30 min)
  utc(1970, 1, 1, 0, 0, 0, 0), // the epoch
  utc(1969, 12, 31, 23, 59, 59, 999), // before the epoch
  utc(2000, 2, 29, 23, 0, 0, 0),
  utc(1900, 3, 1, 0, 0, 0, 0),
  utc(1883, 11, 18, 17, 0, 0, 0), // US railway time
  utc(1850, 6, 15, 12, 0, 0, 0), // before standard time (LMT offsets)
  utc(999, 7, 4, 13, 14, 15, 16), // 3-digit year
  utc(99, 12, 31, 23, 0, 0, 0), // 2-digit year
  utc(5, 5, 5, 5, 5, 5, 5), // 1-digit year
  utc(1, 1, 1, 0, 0, 0, 0), // year 1
  utc(0, 12, 31, 12, 0, 0, 0), // year 0 (1 BC)
  utc(0, 2, 29, 12, 0, 0, 0), // leap day in year 0
  utc(-1, 6, 1, 8, 0, 0, 0), // 2 BC
  utc(-44, 3, 15, 12, 0, 0, 0), // 45 BC
  utc(12345, 6, 7, 8, 9, 10, 11), // 5-digit year
  utc(275760, 9, 13, 0, 0, 0, 0), // the last instant JS holds
  utc(-271821, 4, 20, 0, 0, 0, 0), // the first
  utc(2026, 7, 4, 12, 0, 0, 0), // noon
  utc(2026, 7, 4, 0, 30, 0, 0), // 12:30 AM
  utc(2026, 7, 4, 23, 5, 0, 0),
  utc(2026, 10, 25, 0, 30, 0, 0), // Europe/London DST ends
  utc(1583, 1, 1, 0, 0, 0, 0),
  utc(2100, 2, 28, 0, 0, 0, 0),
];

const ZONES = [
  'utc', 'UTC', 'Utc', 'gmt', 'GMT',
  'UTC+5', 'utc-3:30', 'UTC+05:45', 'UTC-0', 'UTC+0', 'UTC-12', 'UTC+14', 'UTC+9:30', 'utc+00',
  'America/New_York', 'Europe/London', 'Asia/Kolkata', 'Australia/Lord_Howe', 'Pacific/Chatham',
  'Asia/Kathmandu', 'America/St_Johns', 'Pacific/Kiritimati', 'Asia/Tokyo', 'Europe/Moscow',
  'America/Sao_Paulo', 'Pacific/Apia', 'Africa/Casablanca', 'Asia/Tehran', 'America/Los_Angeles',
  'europe/paris', 'AMERICA/CHICAGO', 'Etc/GMT+5', 'Pacific/Honolulu', 'Australia/Sydney', 'Asia/Dubai',
];
const BAD_ZONES = ['Fantasia/Castle', 'Sport~~blorp', 'UTC+', 'UTC+99:99x'];

// Each token on its own.
const TOKENS = [
  'S', 'SSS', 'u', 'uu', 'uuu', 's', 'ss', 'm', 'mm', 'h', 'hh', 'H', 'HH',
  'Z', 'ZZ', 'ZZZ', 'ZZZZ', 'ZZZZZ', 'z', 'a',
  'd', 'dd', 'c', 'ccc', 'cccc', 'ccccc', 'E', 'EEE', 'EEEE', 'EEEEE',
  'L', 'LL', 'LLL', 'LLLL', 'LLLLL', 'M', 'MM', 'MMM', 'MMMM', 'MMMMM',
  'y', 'yy', 'yyyy', 'yyyyyy', 'G', 'GG', 'GGGGG', 'kk', 'kkkk', 'W', 'WW',
  'n', 'nn', 'ii', 'iiii', 'o', 'ooo', 'q', 'qq', 'X', 'x',
  'D', 'DD', 'DDD', 'DDDD', 't', 'tt', 'ttt', 'tttt', 'T', 'TT', 'TTT', 'TTTT',
  'f', 'ff', 'fff', 'ffff', 'F', 'FF', 'FFF', 'FFFF',
];
// Not tokens: pass through.
const NONTOKENS = [
  'yyy', 'yyyyy', 'SS', 'ssss', 'mmm', 'hhh', 'HHH', 'ddd', 'dddd', 'ZZZZZZ', 'zz', 'aa', 'GGG', 'GGGG',
  'kkk', 'WWW', 'ooooo', 'qqq', 'XX', 'xx', 'ttttt', 'DDDDD', 'fffff', 'b', 'B', 'e', 'j', 'K', 'l', 'p', 'r', 'w', 'Y', 'A', 'C', 'e', 'N', 'O', 'P', 'Q',
  'uuuu', 'LLLLLL', 'MMMMMM', 'EEEEEE', 'cccccc', 'nnn', 'iii', 'i', 'k', 'g',
];
const COMBOS = [
  'dd-LLL-yyyy', 'yyyy-MM-dd', 'yyyy-MM-dd HH:mm:ss', "yyyy-MM-dd'T'HH:mm:ss.SSSZZ", 'EEEE, MMMM d, yyyy', 'EEE, dd LLL yyyy HH:mm:ss ZZZ',
  "h:mm a 'on' LLLL d", "'Q'q yyyy", "kkkk-'W'WW-E", 'yyyy-ooo', 'MM/dd/yy', 'M/d/y', "d 'of' MMMM", 'LLL dd, yyyy',
  "hh 'o''clock' a", "''", "'it''s'", "''''", "'", "'abc", "abc'", "'a'b'c'", "a'b", '', ' ', 'yyyy  MM', '  ', 'dd.MM.yyyy', 'yyyy年M月d日',
  "'yyyy'", "'dd' dd", "dd'T'dd", "'' dd ''", "'x y' d", "''d''", "yyyy-MM-dd'  '", "dd LLL yyyy, HH:mm 'UTC'",
  'ddd', 'yyyyMMdd', 'yyMMdd', 'HHmmss', 'D t', 'f f', "DD'T'T", 'ZZZZ ZZZZZ', 'tttt', 'z ZZ', 'X x', 'yyyy-W', 'GG yyyy', 'E c e',
  'MMMM MMM MM M', 'LLLL LLL LL L', 'dddd', 'ZZZZZ ZZZZ ZZZ ZZ Z', 'h a H', "\tHH:mm\n", 'S-SSS-u-uu-uuu', "'literal' LLL", 'LLL.', 'LLL,d',
];
// Dates for tokens that don't depend on the zone.
const SLICE = [0, 1, 2, 3, 4, 7, 9, 12, 14, 15, 19, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33];

const cases = [];
function add(ms, zone, fmt) {
  let out;
  const d = ms === null ? new Date(NaN) : new Date(ms);
  out = DateTime.fromJSDate(d, { zone }).toFormat(fmt);
  cases.push({ ms, zone, fmt, out });
}

// Every token, every date, in UTC.
for (const t of TOKENS) for (const ms of DATES) add(ms, 'utc', t);
// Every token, a few dates, in a handful of zones.
const ZSOME = ['UTC+5', 'utc-3:30', 'America/New_York', 'Asia/Kolkata', 'Pacific/Chatham'];
const DSOME = [DATES[0], DATES[10], DATES[19], DATES[24]];
for (const z of ZSOME) for (const t of TOKENS) for (const ms of DSOME) add(ms, z, t);
// Every zone, with offset/zone tokens and macros.
for (const z of ZONES) for (const t of ['ZZ', 'ZZZZ', 'z', 'tttt', 'HH:mm ZZZ', 'yyyy-MM-dd HH:mm'])
  for (const ms of [DATES[0], DATES[8], DATES[10], DATES[13], DATES[19], DATES[31]]) add(ms, z, t);
// The ends of JavaScript's range, where a zone's offset pushes the date out of it or chrono can't follow.
for (const z of ['utc', 'UTC+5', 'utc-3:30', 'UTC-12', 'UTC+14', 'America/New_York', 'Asia/Tokyo'])
  for (const ms of [DATES[29], DATES[30]]) for (const t of ['yyyy-MM-dd HH:mm', 'ZZ', 'x']) add(ms, z, t);
for (const z of BAD_ZONES) for (const t of ['yyyy', 'ZZ', '']) add(DATES[0], z, t);
// Not tokens, and combinations.
for (const t of NONTOKENS) add(DATES[0], 'utc', t);
for (const t of COMBOS) for (const ms of [DATES[0], DATES[3], DATES[19], DATES[24]]) add(ms, 'utc', t);
for (const t of COMBOS) add(DATES[0], 'America/New_York', t);
// The end of the zone rules chrono-tz has (2099) and after: zones with DST are refused from 2100,
// the rest are fine.
for (const z of ['America/New_York', 'Europe/London', 'Australia/Lord_Howe', 'Asia/Tokyo', 'Asia/Kolkata', 'Pacific/Honolulu', 'America/Phoenix', 'Africa/Casablanca', 'Asia/Tehran', 'UTC+5', 'utc'])
  for (const ms of [utc(2099, 7, 15, 12), utc(2099, 12, 31, 23, 59, 59, 999), utc(2100, 1, 1), utc(2100, 7, 15, 12), utc(2150, 7, 15, 12), utc(2500, 1, 1)])
    for (const t of ['yyyy-MM-dd HH:mm ZZ', 'ZZZZ', 'tttt']) add(ms, z, t);
// Invalid dates.
for (const t of ['yyyy', '', 'dd-LLL-yyyy', "'x'"]) for (const z of ['utc', 'America/New_York']) add(null, z, t);

process.stdout.write(JSON.stringify({ luxon: '3.7.2', cases }, null, 0).replace(/\},\{/g, '},\n{') + '\n');
