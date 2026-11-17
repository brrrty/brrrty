// Generic markdown-it renderer rules, the JavaScript side of the rules differential: the same
// rules are written in Rhai in crates/brrrty-config/tests/markdown_rules.rs. Used by
// gen-markdown-fuzz.cjs (MARKDOWN_RULES=1).
module.exports = function install(md) {
  const rules = md.renderer.rules;
  const renderToken = (tokens, idx, options, env, self) => self.renderToken(tokens, idx, options);

  // A link-tagging rule like the ones sites write: rel and target on matching links.
  const tagged = /(?:^|\/\/)(?:go\.to|(?:www\.)?shop\.)/i;
  const baseLinkOpen = rules.link_open || renderToken;
  rules.link_open = (tokens, idx, options, env, self) => {
    const token = tokens[idx];
    const href = token.attrGet('href') || '';
    if (tagged.test(href)) {
      token.attrSet('rel', 'sponsored nofollow');
      token.attrSet('target', '_blank');
    }
    // Change an href: site-relative links get a host.
    if (href.startsWith('/')) token.attrSet('href', 'https://example.org' + href);
    // Read a neighbor: a title from the link's first text, when it has none.
    const next = tokens[idx + 1];
    if (token.attrIndex('title') < 0 && next && next.type === 'text') token.attrPush(['title', next.content]);
    token.attrJoin('class', 'l' + token.level);
    token.attrJoin('class', token.markup || 'plain');
    return baseLinkOpen(tokens, idx, options, env, self);
  };
  // Wrap a token's output.
  rules.link_close = (tokens, idx, options, env, self) =>
    self.renderToken(tokens, idx, options) + (tokens[idx].info === 'auto' ? '<span class="auto"></span>' : '');
  const baseImage = rules.image;
  rules.image = (tokens, idx, options, env, self) => {
    tokens[idx].attrSet('loading', 'lazy');
    return '<span class="img" data-label="' + md.utils.escapeHtml(tokens[idx].content) + '">' + baseImage(tokens, idx, options, env, self) + '</span>';
  };
  const baseCode = rules.code_inline;
  rules.code_inline = (tokens, idx, options, env, self) => {
    tokens[idx].attrJoin('class', 'a');
    tokens[idx].attrJoin('class', 'b"&');
    return baseCode(tokens, idx, options, env, self);
  };
  const baseText = rules.text;
  rules.text = (tokens, idx, options, env, self) => {
    const t = tokens[idx];
    return (t.info ? '[' + t.info + ':' + md.utils.escapeHtml(t.markup) + ']' : '') + baseText(tokens, idx, options, env, self);
  };
  rules.em_open = (tokens, idx, options, env, self) => '<i' + self.renderAttrs(tokens[idx]) + ' data-m="' + tokens[idx].markup + '">';
  rules.em_close = () => '</i>';
  rules.strong_open = (tokens, idx, options, env, self) => {
    tokens[idx].attrSet('data-level', String(tokens[idx].level));
    return self.renderToken(tokens, idx, options);
  };
  rules.softbreak = () => '<wbr>\n';
  rules.html_inline = (tokens, idx) => md.utils.escapeHtml(tokens[idx].content);
  rules.s_close = (tokens, idx, options, env, self) => {
    tokens[idx].tag = 'del';
    return self.renderToken(tokens, idx, options);
  };
};
