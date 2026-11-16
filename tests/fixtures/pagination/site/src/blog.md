---
pagination:
  data: d.arr
  size: 2
  alias: posts
permalink: "/blog{% if pagination.pageNumber > 0 %}/{{ pagination.pageNumber }}{% endif %}/"
title: Blog
---
# {{ title }} {{ posts | join: "," }}

{% if pagination.href.previous %}<a href="{{ pagination.href.previous }}">prev</a>{% endif %}
{% if pagination.href.next %}<a href="{{ pagination.href.next }}">next</a>{% endif %}
links={{ pagination.links | json }}
