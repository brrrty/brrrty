---
layout: show
templateEngineOverride: liquid,md
eleventyComputed:
  v: "{{ 1 | plus: 2 }} {% if true %}t{% endif %}"
---
body
