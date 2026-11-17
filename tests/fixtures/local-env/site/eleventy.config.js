export default function (eleventyConfig) {
  // The parts of a local date: new Date(y, m, d, h, mi) and its getters.
  eleventyConfig.addFilter('localParts', (args) => {
    const d = new Date(...args);
    return [d.getFullYear(), d.getMonth(), d.getDate(), d.getDay(), d.getHours(), d.getMinutes(), d.getTimezoneOffset(), d.toISOString()].join(' ');
  });
  eleventyConfig.addFilter('num', (n) => n.toLocaleString());
  eleventyConfig.addFilter('enNum', (n) => n.toLocaleString('en-US'));
  eleventyConfig.addFilter('sortLocale', (arr) => [...arr].sort((a, b) => a.localeCompare(b)).join(' '));
  eleventyConfig.addFilter('sortEn', (arr) => [...arr].sort((a, b) => a.localeCompare(b, 'en-US')).join(' '));
  return { dir: { input: 'src' } };
}
