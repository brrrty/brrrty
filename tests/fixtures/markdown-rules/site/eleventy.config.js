// Renderer rules on 11ty's own markdown-it, the shapes sites use: tag some
// links, add a class, lazy-load images, wrap code, mark external links.
const TAGGED = /(?:^|\/\/)(?:go\.to|(?:www\.)?shop\.)/i;

function linkRules(md) {
  const base = md.renderer.rules.link_open
    || ((tokens, idx, options, env, self) => self.renderToken(tokens, idx, options));
  md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
    const href = tokens[idx].attrGet('href') || '';
    if (TAGGED.test(href)) {
      tokens[idx].attrSet('rel', 'sponsored nofollow');
      tokens[idx].attrSet('target', '_blank');
    }
    if (/^https?:/.test(href)) tokens[idx].attrJoin('class', 'external');
    return base(tokens, idx, options, env, self);
  };
  md.renderer.rules.link_close = (tokens, idx, options, env, self) =>
    (tokens[idx].markup === 'autolink' ? '<span class="auto"></span>' : '') + self.renderToken(tokens, idx, options);
  return md;
}

module.exports = function (eleventyConfig) {
  eleventyConfig.amendLibrary('md', (md) => linkRules(md));
  eleventyConfig.amendLibrary('md', (md) => {
    const image = md.renderer.rules.image;
    md.renderer.rules.image = (tokens, idx, options, env, self) => {
      tokens[idx].attrSet('loading', 'lazy');
      return image(tokens, idx, options, env, self);
    };
    const code = md.renderer.rules.code_inline;
    md.renderer.rules.code_inline = (tokens, idx, options, env, self) =>
      '<kbd>' + code(tokens, idx, options, env, self) + '</kbd>';
  });

  return {
    dir: {
      input: "src",
    },
  };
};
