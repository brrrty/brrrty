---
date: 2023-01-01
pagination:
  data: collections.meta.posts.tags
  size: 1
  alias: tag
permalink: "/tag/{{ pagination.pageNumber }}/"
---
# Tag: {{ tag }}

{% for post in collections.meta.posts.tags[tag] -%}
- {{ post.data.title }} {{ post.url }}
{% endfor %}
