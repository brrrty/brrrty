---
date: 2023-01-01
tags: [listing]
pagination:
  data: collections.posts
  size: 2
  alias: entries
permalink: "/blog/{{ pagination.pageNumber }}/"
---
# Blog {{ pagination.pageNumber }}

{% for post in entries -%}
## {{ post.data.title }} ({{ post.url }})

{{ post.templateContent }}
{% endfor %}
