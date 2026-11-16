// Regenerates tests/probes/nunjucks_filters.json: templates that use the
// filters Eleventy adds to Nunjucks, rendered by Nunjucks 3.2.4 with those
// filters registered as Eleventy and @11ty/eleventy-plugin-rss 3.1.0 add
// them (`slug`, `slugify`, `url`, `log` from Eleventy's default config, and
// the plugin's filters).
//
//   NUNJUCKS=/path/to/node_modules/nunjucks \
//   ELEVENTY=/path/to/node_modules/@11ty/eleventy \
//   RSS=/path/to/node_modules/@11ty/eleventy-plugin-rss \
//   TZ=UTC node tools/gen-nunjucks-filter-probes.cjs > tests/probes/nunjucks_filters.json
//
// Run it from crates/brrrty-core. `htmlToAbsoluteUrls` is an async filter
// in the plugin; Brrrty refuses it, so it isn't probed here.

const path = require('path');
const nunjucks = require(process.env.NUNJUCKS);

const data = {
  d: '__DATE__2020-05-05T10:20:30.123Z',
  d0: '__DATE__1970-01-01T00:00:00.000Z',
  posts: [
    { url: '/a/', date: '__DATE__2020-01-01T00:00:00Z' },
    { url: '/b/', date: '__DATE__2021-03-04T05:06:07Z' },
    { url: '/c/', date: '__DATE__2019-12-31T23:59:59Z' },
  ],
  n: 0, s: 'text', u8: '/p/q?x=1', base: 'https://example.com/blog/',
};
const templates = [
  '{{ d | dateToRfc3339 }}', '{{ d0 | dateToRfc3339 }}', '{{ d | dateToRfc822 }}', '{{ d0 | dateToRfc822 }}',
  '{{ n | dateToRfc822 }}', '{{ posts[0].date | dateToRfc822 }}',
  '{{ posts | getNewestCollectionItemDate | dateToRfc3339 }}', '{{ posts | getNewestCollectionItemDate | dateToRfc822 }}',
  '{{ posts | getNewestCollectionItemDate }}',
  '{{ [] | getNewestCollectionItemDate("fallback") }}', '{{ undefined | getNewestCollectionItemDate("fallback") }}',
  '{{ posts | eleventyFeedHasUrl | length }}',
  '{{ u8 | absoluteUrl(base) }}', '{{ "../x" | absoluteUrl(base) }}', '{{ "https://other.org/y" | absoluteUrl(base) }}',
  '{{ "bad" | absoluteUrl("nope") }}', '{{ "http://a.com/x y" | absoluteUrl }}', '{{ undefined | absoluteUrl("https://x.com") }}',
  '{{ "/x" | absoluteUrl }}', '{{ 5 | absoluteUrl(base) }}',
  '{{ "x" | dateToRfc3339 }}', '{{ undefined | dateToRfc3339 }}', '{{ null | dateToRfc822 }}',
  '{{ "A b" | slug }}', '{{ "A b" | slugify }}', '{{ "a/b" | url }}', '{{ "x" | log }}',
  '{{ "x" | rssDate }}', '{{ "x" | rssLastUpdatedDate }}',
  '{{ "x" | nosuchfilter }}',
  // SafeString inputs and arguments.
  '{{ ("a&b" | safe) | absoluteUrl("nope") }}', '{{ ("a&b" | safe) | absoluteUrl }}', '{{ ("a&b" | safe) | absoluteUrl(base) }}',
  '{{ ("/a&b" | safe) | absoluteUrl(base) }}', '{{ ("https://x.com/" | safe) | absoluteUrl }}', '{{ ("https://x.com/a&b" | safe) | absoluteUrl | dump }}',
  '{{ ("a&b" | safe) | absoluteUrl("nope") | dump }}', '{{ "a" | absoluteUrl(("https://x.com/" | safe)) }}',
  '{{ ("<x>" | safe) | getNewestCollectionItemDate }}', '{{ ("" | safe) | getNewestCollectionItemDate("<f>") }}',
  '{{ [] | getNewestCollectionItemDate(("<x>" | safe)) }}', '{{ [] | getNewestCollectionItemDate(("<x>" | safe)) | dump }}', '{{ [] | getNewestCollectionItemDate("<x>") | dump }}',
  '{{ ("<x>" | safe) | dateToRfc3339 }}', '{{ ("<x>" | safe) | dateToRfc822 }}',
  '{{ ("A <b>" | safe) | slug }}', '{{ ("A <b>" | safe) | slugify | dump }}', '{{ ("/a/<b>" | safe) | url }}', '{{ ("<b>" | safe) | log | dump }}',
];

const dates = (v) => {
  if (typeof v === 'string' && v.startsWith('__DATE__')) return new Date(v.slice(8));
  if (Array.isArray(v)) return v.map(dates);
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, dates(x)]));
  return v;
};

(async () => {
  const env = new nunjucks.Environment(null, { dev: true });
  const base = path.join(process.env.ELEVENTY, 'src/Filters');
  const url = (await import(path.join(base, 'Url.js'))).default;
  env.addFilter('slug', (await import(path.join(base, 'Slug.js'))).default);
  env.addFilter('slugify', (await import(path.join(base, 'Slugify.js'))).default);
  env.addFilter('url', (u, o) => url(u, o && typeof o === 'string' ? o : '/'));
  env.addFilter('log', (i) => i);
  const rss = path.join(process.env.RSS, 'src');
  env.addFilter('getNewestCollectionItemDate', (await import(path.join(rss, 'getNewestCollectionItemDate.js'))).default);
  env.addFilter('dateToRfc3339', (await import(path.join(rss, 'dateRfc3339.js'))).default);
  env.addFilter('dateToRfc822', (await import(path.join(rss, 'dateRfc822.js'))).default);
  env.addFilter('absoluteUrl', (await import(path.join(rss, 'absoluteUrl.js'))).default);
  env.addFilter('eleventyFeedHasUrl', (array) => array.filter((entry) => entry.url));
  env.addFilter('rssLastUpdatedDate', () => { throw new Error('The `rssLastUpdatedDate` filter was removed. Use `getNewestCollectionItemDate | dateToRfc3339` (for Atom) or `getNewestCollectionItemDate | dateToRfc822` (for RSS) instead.'); });
  env.addFilter('rssDate', () => { throw new Error('The `rssDate` filter was removed. Use `dateToRfc3339` (for Atom) or `dateToRfc822` (for RSS) instead.'); });
  const probes = templates.map((t) => {
    const e = { t };
    try { e.out = env.renderString(t, dates(JSON.parse(JSON.stringify(data)))); } catch (x) {
      const parts = String(x.message).split('\n  ');
      e.err = parts[parts.length - 1];
    }
    return e;
  });
  process.stdout.write(JSON.stringify({ data, probes }, null, 1) + '\n');
})();
