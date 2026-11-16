---
date: 2023-01-01
pagination:
  data: collections.meta.posts.categories
  size: 1
  alias: category
permalink: "/category/{{ pagination.pageNumber }}/"
---
# Category: {{ category }}

{% for post in collections.meta.posts.categories[category] | reverse -%}
- {{ post.data.title }} {{ post.url }}: {{ post.content | strip_html | strip }}
{% endfor %}
