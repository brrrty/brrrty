---
date: 2023-01-02
pagination:
  data: collections.listing
  size: 1
permalink: "meta/{{ pagination.pageNumber }}/"
---
{% for i in pagination.items %}{{ i.url }} {% endfor %}
