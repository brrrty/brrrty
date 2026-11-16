---
tags: [tb, post]
date: 2020-01-02
eleventyImport:
  collections: [post, tz]
---
B{% for k in collections %}{{ k[0] }},{% endfor %}|{% for p in collections.post %}{{ p.templateContent | size }}{% endfor %}
